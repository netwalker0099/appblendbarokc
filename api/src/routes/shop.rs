//! The online shop: listings the admin manages, the public catalogue the
//! customer site renders, and the checkout that turns a bag into a Square
//! payment link.
//!
//! A listing is either a standalone **product** with its own price, or a house
//! **scent** whose prices are read from the catalogue at checkout — see
//! migration 0024 for why the two share a table.
//!
//! The checkout follows the share-page checkout (`public::checkout`) point for
//! point, and the same rules hold for the same reasons: prices never come from
//! the request, an existing customer row is never modified, no marketing
//! opt-in, rate limited per IP, and refused outright unless Square is in
//! production.

use std::collections::HashMap;

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::models::cart::Cart;
use crate::models::customer::Customer;
use crate::models::order::{BottleSize, Order, OrderStatus, OrderType};
use crate::models::scent::Scent;
use crate::models::sync::SyncEntity;
use crate::ratelimit::client_key;
use crate::routes::public::{ensure_public_checkout_live, public_checkout_live, site_url};
use crate::square::money::{self, DEFAULT_CURRENCY};
use crate::square::{CheckoutPush, DiscountPush, LineItemPush};
use crate::AppState;

/// Product photos. Phone photos are routinely 3–4MB; anything larger should be
/// resized before it is put on a web page anyway.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Distinct lines in one shop order. Generous for a boutique; the point is that
/// an anonymous request cannot ask for ten thousand rows.
const MAX_LINES: usize = 20;
/// Per line. Each scent bottle becomes its own order for staff to make up, so
/// this also bounds how many order rows one request can create.
const MAX_QUANTITY: i32 = 10;
/// Across the whole bag. Lines × quantity alone would allow 200 bottles, each
/// an order row; no real online order is near this.
const MAX_UNITS: i32 = 30;

const NAME_MAX: usize = 120;
const DESCRIPTION_MAX: usize = 2000;

// --- Rows -------------------------------------------------------------------

#[derive(Debug, sqlx::FromRow)]
struct ProductRow {
    id: Uuid,
    kind: String,
    name: Option<String>,
    description: Option<String>,
    price: Option<Decimal>,
    scent_id: Option<Uuid>,
    image_id: Option<Uuid>,
    active: bool,
    sold_out: bool,
    position: i32,
}

const PRODUCT_COLS: &str =
    "id, kind, name, description, price, scent_id, image_id, active, sold_out, position";

fn image_url(id: Option<Uuid>) -> Option<String> {
    id.map(|id| format!("/api/public/shop/images/{id}"))
}

fn scent_price(scent: &Scent, size: BottleSize) -> Option<Decimal> {
    match size {
        BottleSize::Vintage => scent.price_vintage,
        BottleSize::Oz3_4 => scent.price_oz3_4,
        BottleSize::Oz1_7 => scent.price_oz1_7,
        BottleSize::Oz1_2 => scent.price_oz1_2,
        BottleSize::Roller => scent.price_roller,
        BottleSize::Spray => scent.price_spray,
    }
    .filter(|p| *p > Decimal::ZERO)
}

// --- Admin: listings --------------------------------------------------------

#[derive(Serialize)]
pub struct AdminProduct {
    pub id: Uuid,
    pub kind: String,
    /// What the shop shows: the listing's own name, or the scent's.
    pub display_name: String,
    /// The stored name — null on a scent listing that follows the scent's name.
    pub name: Option<String>,
    pub description: Option<String>,
    pub price: Option<Decimal>,
    pub scent_id: Option<Uuid>,
    pub scent_active: Option<bool>,
    pub image_id: Option<Uuid>,
    pub image_url: Option<String>,
    pub active: bool,
    pub sold_out: bool,
    pub position: i32,
    /// Units on carts that have been paid for.
    pub units_sold: i64,
}

#[derive(sqlx::FromRow)]
struct AdminProductRow {
    #[sqlx(flatten)]
    row: ProductRow,
    scent_name: Option<String>,
    scent_active: Option<bool>,
    units_sold: i64,
}

async fn load_admin_products(
    db: &sqlx::PgPool,
    only: Option<Uuid>,
) -> Result<Vec<AdminProduct>, AppError> {
    let rows = sqlx::query_as::<_, AdminProductRow>(
        r#"
        select p.id, p.kind, p.name, p.description, p.price, p.scent_id, p.image_id,
               p.active, p.sold_out, p.position,
               s.name as scent_name, s.active as scent_active,
               coalesce((
                   select sum(ci.quantity) from cart_items ci
                   join carts c on c.id = ci.cart_id
                   where ci.shop_product_id = p.id and c.status = 'paid'
               ), 0)::bigint as units_sold
        from shop_products p
        left join scents s on s.id = p.scent_id
        where p.archived_at is null and ($1::uuid is null or p.id = $1)
        order by p.position, p.created_at
        "#,
    )
    .bind(only)
    .fetch_all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| AdminProduct {
            id: r.row.id,
            display_name: r
                .row
                .name
                .clone()
                .or(r.scent_name)
                .unwrap_or_else(|| "Untitled".into()),
            kind: r.row.kind,
            name: r.row.name,
            description: r.row.description,
            price: r.row.price,
            scent_id: r.row.scent_id,
            scent_active: r.scent_active,
            image_url: image_url(r.row.image_id),
            image_id: r.row.image_id,
            active: r.row.active,
            sold_out: r.row.sold_out,
            position: r.row.position,
            units_sold: r.units_sold,
        })
        .collect())
}

pub async fn list(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<Vec<AdminProduct>>, AppError> {
    Ok(Json(load_admin_products(&state.db, None).await?))
}

#[derive(Deserialize)]
pub struct UpsertProduct {
    pub kind: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub price: Option<Decimal>,
    pub scent_id: Option<Uuid>,
    pub image_id: Option<Uuid>,
    pub active: Option<bool>,
    pub sold_out: Option<bool>,
}

/// The validated form of [`UpsertProduct`], with the fields that don't apply to
/// the listing's kind cleared rather than rejected — the admin form keeps both
/// sets of inputs around while someone flips between them.
struct CleanProduct {
    kind: &'static str,
    name: Option<String>,
    description: Option<String>,
    price: Option<Decimal>,
    scent_id: Option<Uuid>,
    image_id: Option<Uuid>,
}

fn trimmed(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

async fn clean(db: &sqlx::PgPool, body: &UpsertProduct) -> Result<CleanProduct, AppError> {
    let name = trimmed(body.name.as_deref());
    let description = trimmed(body.description.as_deref());
    if name.as_ref().is_some_and(|n| n.chars().count() > NAME_MAX) {
        return Err(AppError::BadRequest(format!(
            "names are limited to {NAME_MAX} characters"
        )));
    }
    if description
        .as_ref()
        .is_some_and(|d| d.chars().count() > DESCRIPTION_MAX)
    {
        return Err(AppError::BadRequest(format!(
            "descriptions are limited to {DESCRIPTION_MAX} characters"
        )));
    }

    if let Some(image_id) = body.image_id {
        let exists: bool =
            sqlx::query_scalar("select exists (select 1 from shop_images where id = $1)")
                .bind(image_id)
                .fetch_one(db)
                .await?;
        if !exists {
            return Err(AppError::BadRequest(
                "that photo is no longer available — upload it again".into(),
            ));
        }
    }

    match body.kind.as_str() {
        "product" => {
            let name =
                name.ok_or_else(|| AppError::BadRequest("a product needs a name".into()))?;
            let price = body
                .price
                .ok_or_else(|| AppError::BadRequest("a product needs a price".into()))?;
            if price <= Decimal::ZERO {
                return Err(AppError::BadRequest("price must be more than zero".into()));
            }
            // Square takes whole cents. The column would round $1.005 to $1.01
            // without a word, so refuse it here instead.
            if price.normalize().scale() > 2 || money::to_cents(price).is_none() {
                return Err(AppError::BadRequest("that price isn't a valid amount".into()));
            }
            Ok(CleanProduct {
                kind: "product",
                name: Some(name),
                description,
                price: Some(price),
                scent_id: None,
                image_id: body.image_id,
            })
        }
        "scent" => {
            let scent_id = body
                .scent_id
                .ok_or_else(|| AppError::BadRequest("choose a scent to list".into()))?;
            let exists: bool =
                sqlx::query_scalar("select exists (select 1 from scents where id = $1)")
                    .bind(scent_id)
                    .fetch_one(db)
                    .await?;
            if !exists {
                return Err(AppError::BadRequest("that scent doesn't exist".into()));
            }
            Ok(CleanProduct {
                kind: "scent",
                name,
                description,
                price: None,
                scent_id: Some(scent_id),
                image_id: body.image_id,
            })
        }
        other => Err(AppError::BadRequest(format!(
            "unknown listing kind '{other}' — expected product or scent"
        ))),
    }
}

fn map_write_error(e: sqlx::Error) -> AppError {
    match &e {
        sqlx::Error::Database(db) if db.is_unique_violation() => {
            AppError::Conflict("that scent is already listed in the shop".into())
        }
        _ => AppError::from(e),
    }
}

/// Delete a photo nobody points at any more. Called when a listing's photo is
/// replaced or the listing is removed, so the table only holds pictures in use.
async fn drop_image_if_unused(db: &sqlx::PgPool, image_id: Option<Uuid>) -> Result<(), AppError> {
    if let Some(id) = image_id {
        sqlx::query(
            "delete from shop_images where id = $1 \
             and not exists (select 1 from shop_products where image_id = $1)",
        )
        .bind(id)
        .execute(db)
        .await?;
    }
    Ok(())
}

pub async fn create(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<UpsertProduct>,
) -> Result<(StatusCode, Json<AdminProduct>), AppError> {
    let p = clean(&state.db, &body).await?;

    // New listings go to the end of the shop.
    let id: Uuid = sqlx::query_scalar(
        r#"
        insert into shop_products
            (kind, name, description, price, scent_id, image_id, active, sold_out, position)
        values ($1, $2, $3, $4, $5, $6, coalesce($7, true), coalesce($8, false),
                (select coalesce(max(position) + 1, 0) from shop_products where archived_at is null))
        returning id
        "#,
    )
    .bind(p.kind)
    .bind(&p.name)
    .bind(&p.description)
    .bind(p.price)
    .bind(p.scent_id)
    .bind(p.image_id)
    .bind(body.active)
    .bind(body.sold_out)
    .fetch_one(&state.db)
    .await
    .map_err(map_write_error)?;

    let product = load_admin_products(&state.db, Some(id))
        .await?
        .pop()
        .ok_or_else(|| AppError::Internal("created listing vanished".into()))?;
    Ok((StatusCode::CREATED, Json(product)))
}

pub async fn update(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpsertProduct>,
) -> Result<Json<AdminProduct>, AppError> {
    let p = clean(&state.db, &body).await?;

    let previous_image: Option<Option<Uuid>> = sqlx::query_scalar(
        "select image_id from shop_products where id = $1 and archived_at is null",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some(previous_image) = previous_image else {
        return Err(AppError::NotFound("listing not found".into()));
    };

    sqlx::query(
        r#"
        update shop_products set
            kind = $2, name = $3, description = $4, price = $5, scent_id = $6,
            image_id = $7,
            active = coalesce($8, active),
            sold_out = coalesce($9, sold_out),
            updated_at = now()
        where id = $1 and archived_at is null
        "#,
    )
    .bind(id)
    .bind(p.kind)
    .bind(&p.name)
    .bind(&p.description)
    .bind(p.price)
    .bind(p.scent_id)
    .bind(p.image_id)
    .bind(body.active)
    .bind(body.sold_out)
    .execute(&state.db)
    .await
    .map_err(map_write_error)?;

    if previous_image != p.image_id {
        drop_image_if_unused(&state.db, previous_image).await?;
    }

    let product = load_admin_products(&state.db, Some(id))
        .await?
        .pop()
        .ok_or_else(|| AppError::NotFound("listing not found".into()))?;
    Ok(Json(product))
}

#[derive(Serialize)]
pub struct RemoveResult {
    /// True when the listing had sales and was archived instead of deleted.
    pub archived: bool,
}

/// Take a listing out of the shop.
///
/// Always succeeds from the admin's point of view — "remove" should remove. A
/// listing that has never been on a cart is deleted outright; one that has is
/// archived, so the cart lines that name it keep a valid reference and the
/// sales history stays intact. Either way it is gone from the shop and from the
/// admin list.
pub async fn remove(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<RemoveResult>, AppError> {
    let image_id: Option<Option<Uuid>> = sqlx::query_scalar(
        "select image_id from shop_products where id = $1 and archived_at is null",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let Some(image_id) = image_id else {
        return Err(AppError::NotFound("listing not found".into()));
    };

    let referenced: bool =
        sqlx::query_scalar("select exists (select 1 from cart_items where shop_product_id = $1)")
            .bind(id)
            .fetch_one(&state.db)
            .await?;

    if referenced {
        // The photo is kept: an archived listing still points at it, and it is
        // what someone looking back at the sale would expect to see.
        sqlx::query(
            "update shop_products set archived_at = now(), active = false, updated_at = now() \
             where id = $1",
        )
        .bind(id)
        .execute(&state.db)
        .await?;
    } else {
        sqlx::query("delete from shop_products where id = $1")
            .bind(id)
            .execute(&state.db)
            .await?;
        drop_image_if_unused(&state.db, image_id).await?;
    }

    Ok(Json(RemoveResult {
        archived: referenced,
    }))
}

#[derive(Deserialize)]
pub struct ReorderRequest {
    /// Every live listing's id, in the order the shop should show them.
    pub ids: Vec<Uuid>,
}

pub async fn reorder(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<ReorderRequest>,
) -> Result<StatusCode, AppError> {
    let mut tx = state.db.begin().await?;
    for (i, id) in body.ids.iter().enumerate() {
        sqlx::query("update shop_products set position = $2 where id = $1 and archived_at is null")
            .bind(id)
            .bind(i as i32)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- Admin: photos ----------------------------------------------------------

#[derive(Serialize)]
pub struct UploadedImage {
    pub id: Uuid,
    pub url: String,
}

/// Identify an image by its first bytes. The browser's Content-Type is not
/// trusted: whatever is stored here is served back to the public with the type
/// recorded, so it has to be what it claims.
fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// Upload a product photo. The body is the raw image file.
///
/// Uploaded first and attached to a listing on save, so the admin sees the
/// picture before committing to it. A photo uploaded and then abandoned is
/// cleared out by the sweep below on a later upload.
pub async fn upload_image(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    body: Bytes,
) -> Result<(StatusCode, Json<UploadedImage>), AppError> {
    if body.is_empty() {
        return Err(AppError::BadRequest("no image was sent".into()));
    }
    if body.len() > MAX_IMAGE_BYTES {
        return Err(AppError::BadRequest(
            "that photo is over 5MB — please use a smaller one".into(),
        ));
    }
    let content_type = sniff_image(&body).ok_or_else(|| {
        AppError::BadRequest("photos must be JPEG, PNG or WebP".into())
    })?;

    // Abandoned uploads: a day is long enough that nobody is still mid-edit.
    sqlx::query(
        "delete from shop_images where created_at < now() - interval '1 day' \
         and not exists (select 1 from shop_products p where p.image_id = shop_images.id)",
    )
    .execute(&state.db)
    .await?;

    let id: Uuid = sqlx::query_scalar(
        "insert into shop_images (content_type, bytes) values ($1, $2) returning id",
    )
    .bind(content_type)
    .bind(body.as_ref())
    .fetch_one(&state.db)
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(UploadedImage {
            id,
            url: image_url(Some(id)).unwrap_or_default(),
        }),
    ))
}

/// Serve a product photo. Public — the shop page needs it.
///
/// An image id is never reused and an image is never edited in place (a new
/// photo is a new upload), so the response can be cached indefinitely.
pub async fn image(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, AppError> {
    let row: Option<(String, Vec<u8>)> =
        sqlx::query_as("select content_type, bytes from shop_images where id = $1")
            .bind(id)
            .fetch_optional(&state.db)
            .await?;
    let (content_type, bytes) = row.ok_or_else(|| AppError::NotFound("image not found".into()))?;

    Response::builder()
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .body(Body::from(bytes))
        .map_err(|e| AppError::Internal(e.to_string()))
}

// --- Admin: shipping --------------------------------------------------------

#[derive(Serialize, Deserialize, sqlx::FromRow)]
pub struct ShopSettings {
    pub shop_shipping_cents: i64,
    /// Null means shipping is always charged.
    pub shop_free_shipping_over_cents: Option<i64>,
}

async fn load_settings(db: &sqlx::PgPool) -> Result<ShopSettings, AppError> {
    Ok(sqlx::query_as::<_, ShopSettings>(
        "select shop_shipping_cents, shop_free_shipping_over_cents from settings where id = true",
    )
    .fetch_one(db)
    .await?)
}

pub async fn get_settings(
    _admin: AdminEmployee,
    State(state): State<AppState>,
) -> Result<Json<ShopSettings>, AppError> {
    Ok(Json(load_settings(&state.db).await?))
}

/// Both fields are always sent together by the Shop tab, so this is a plain
/// replace rather than a partial patch.
pub async fn update_settings(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<ShopSettings>,
) -> Result<Json<ShopSettings>, AppError> {
    if body.shop_shipping_cents < 0 {
        return Err(AppError::BadRequest("shipping can't be negative".into()));
    }
    if body.shop_free_shipping_over_cents.is_some_and(|c| c <= 0) {
        return Err(AppError::BadRequest(
            "the free-shipping threshold must be more than zero — leave it blank to always charge"
                .into(),
        ));
    }
    let s = sqlx::query_as::<_, ShopSettings>(
        "update settings set shop_shipping_cents = $1, shop_free_shipping_over_cents = $2 \
         where id = true returning shop_shipping_cents, shop_free_shipping_over_cents",
    )
    .bind(body.shop_shipping_cents)
    .bind(body.shop_free_shipping_over_cents)
    .fetch_one(&state.db)
    .await?;
    Ok(Json(s))
}

fn shipping_for(settings: &ShopSettings, subtotal_cents: i64) -> i64 {
    match settings.shop_free_shipping_over_cents {
        Some(threshold) if subtotal_cents >= threshold => 0,
        _ => settings.shop_shipping_cents,
    }
}

// --- Public catalogue -------------------------------------------------------

#[derive(Serialize)]
pub struct PublicSizeOption {
    pub size: BottleSize,
    pub label: &'static str,
    pub price: Decimal,
}

#[derive(Serialize)]
pub struct PublicProduct {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub sold_out: bool,
    /// Set for a product.
    pub price: Option<Decimal>,
    /// Set for a scent: the sizes it is sold in, largest first.
    pub sizes: Vec<PublicSizeOption>,
    /// Ingredient names for a scent — never amounts, as on the share page.
    pub notes: Vec<String>,
}

#[derive(Serialize)]
pub struct PublicShop {
    pub products: Vec<PublicProduct>,
    pub shipping_cents: i64,
    pub free_shipping_over_cents: Option<i64>,
    /// False until Square is in production. The page still shows the shop, but
    /// says checkout isn't open rather than letting someone fill a bag for
    /// nothing.
    pub checkout_open: bool,
}

/// Load one live listing with what the public may see of it. `None` when it is
/// hidden, archived, or a scent that is inactive or has no prices — the same
/// test the public list applies, so a bag can't hold anything the page
/// wouldn't show.
async fn public_product(
    db: &sqlx::PgPool,
    row: ProductRow,
) -> Result<Option<(PublicProduct, Option<Scent>)>, AppError> {
    let kind = row.kind.clone();
    match kind.as_str() {
        "product" => {
            let (Some(name), Some(price)) = (row.name, row.price) else {
                return Ok(None);
            };
            Ok(Some((
                PublicProduct {
                    id: row.id,
                    kind: row.kind,
                    name,
                    description: row.description,
                    image_url: image_url(row.image_id),
                    sold_out: row.sold_out,
                    price: Some(price),
                    sizes: Vec::new(),
                    notes: Vec::new(),
                },
                None,
            )))
        }
        "scent" => {
            let Some(scent_id) = row.scent_id else {
                return Ok(None);
            };
            let scent = sqlx::query_as::<_, Scent>(
                "select * from scents where id = $1 and active = true",
            )
            .bind(scent_id)
            .fetch_optional(db)
            .await?;
            let Some(scent) = scent else {
                return Ok(None);
            };
            let sizes: Vec<PublicSizeOption> = BottleSize::ALL
                .into_iter()
                .filter_map(|size| {
                    scent_price(&scent, size).map(|price| PublicSizeOption {
                        size,
                        label: size.label(),
                        price,
                    })
                })
                .collect();
            if sizes.is_empty() {
                return Ok(None);
            }
            let notes: Vec<String> = sqlx::query_scalar(
                "select i.name from scent_items si join ingredients i on i.id = si.ingredient_id \
                 where si.scent_id = $1 order by i.name",
            )
            .bind(scent_id)
            .fetch_all(db)
            .await?;
            Ok(Some((
                PublicProduct {
                    id: row.id,
                    kind: row.kind,
                    name: row.name.unwrap_or_else(|| scent.name.clone()),
                    description: row.description,
                    image_url: image_url(row.image_id),
                    sold_out: row.sold_out,
                    price: None,
                    sizes,
                    notes,
                },
                Some(scent),
            )))
        }
        _ => Ok(None),
    }
}

pub async fn catalogue(State(state): State<AppState>) -> Result<Json<PublicShop>, AppError> {
    let rows = sqlx::query_as::<_, ProductRow>(&format!(
        "select {PRODUCT_COLS} from shop_products \
         where active = true and archived_at is null order by position, created_at"
    ))
    .fetch_all(&state.db)
    .await?;

    let mut products = Vec::with_capacity(rows.len());
    for row in rows {
        if let Some((p, _)) = public_product(&state.db, row).await? {
            products.push(p);
        }
    }

    let settings = load_settings(&state.db).await?;
    Ok(Json(PublicShop {
        products,
        shipping_cents: settings.shop_shipping_cents,
        free_shipping_over_cents: settings.shop_free_shipping_over_cents,
        checkout_open: public_checkout_live(&state),
    }))
}

// --- Public checkout --------------------------------------------------------

#[derive(Deserialize)]
pub struct BagLine {
    pub product_id: Uuid,
    /// Required for a scent listing, ignored for a product.
    pub size: Option<BottleSize>,
    pub quantity: i32,
}

#[derive(Deserialize)]
pub struct ShopCheckoutRequest {
    pub items: Vec<BagLine>,
    pub email: String,
    pub name: Option<String>,
    pub coupon_code: Option<String>,
}

#[derive(Serialize)]
pub struct ShopCheckoutResponse {
    pub checkout_url: String,
    pub subtotal_cents: i64,
    pub shipping_cents: i64,
    pub discount_cents: i64,
    pub total_cents: i64,
}

/// A bag line after pricing: everything needed to write it and send it on.
struct PricedLine {
    product_id: Uuid,
    name: String,
    quantity: i32,
    unit_cents: i64,
    /// Set for a scent bottle — it becomes an order for staff to make up.
    scent: Option<(Uuid, BottleSize, Decimal)>,
}

/// Check out a shop bag. **No authentication** — see the module docs for the
/// rules this follows and why.
pub async fn checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ShopCheckoutRequest>,
) -> Result<Json<ShopCheckoutResponse>, AppError> {
    // Shares the share page's limiter: both are the same anonymous
    // "write rows and call Square" action, so one budget covers either.
    if !state.public_checkout_limiter.check(&client_key(&headers)) {
        return Err(AppError::TooManyRequests(
            "too many checkout attempts — please wait a minute and try again".into(),
        ));
    }

    let email = body.email.trim().to_lowercase();
    if email.len() > 254 || !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(AppError::BadRequest("a valid email is required".into()));
    }
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(100).collect::<String>());

    if body.items.is_empty() {
        return Err(AppError::BadRequest("your bag is empty".into()));
    }
    if body.items.len() > MAX_LINES {
        return Err(AppError::BadRequest(format!(
            "a bag is limited to {MAX_LINES} different items"
        )));
    }

    let units: i32 = body.items.iter().map(|i| i.quantity.clamp(0, MAX_QUANTITY)).sum();
    if units > MAX_UNITS {
        return Err(AppError::BadRequest(format!(
            "online orders are limited to {MAX_UNITS} items — message us for a larger order"
        )));
    }

    // Price every line from the database. The request chooses what and how
    // many; it never chooses what anything costs.
    let mut cache: HashMap<Uuid, Option<(PublicProduct, Option<Scent>)>> = HashMap::new();
    let mut lines: Vec<PricedLine> = Vec::with_capacity(body.items.len());
    for item in &body.items {
        if !(1..=MAX_QUANTITY).contains(&item.quantity) {
            return Err(AppError::BadRequest(format!(
                "quantities must be between 1 and {MAX_QUANTITY}"
            )));
        }
        if !cache.contains_key(&item.product_id) {
            let row = sqlx::query_as::<_, ProductRow>(&format!(
                "select {PRODUCT_COLS} from shop_products \
                 where id = $1 and active = true and archived_at is null"
            ))
            .bind(item.product_id)
            .fetch_optional(&state.db)
            .await?;
            let loaded = match row {
                Some(row) => public_product(&state.db, row).await?,
                None => None,
            };
            cache.insert(item.product_id, loaded);
        }
        let Some((product, scent)) = cache.get(&item.product_id).and_then(Option::as_ref) else {
            return Err(AppError::BadRequest(
                "something in your bag is no longer available — please refresh the shop".into(),
            ));
        };
        if product.sold_out {
            return Err(AppError::BadRequest(format!(
                "{} is sold out — please remove it from your bag",
                product.name
            )));
        }

        match scent {
            None => {
                let price = product.price.unwrap_or_default();
                let unit_cents = money::to_cents(price).ok_or_else(|| {
                    AppError::Internal(format!("listing {} has an invalid price", product.id))
                })?;
                lines.push(PricedLine {
                    product_id: product.id,
                    name: product.name.clone(),
                    quantity: item.quantity,
                    unit_cents,
                    scent: None,
                });
            }
            Some(scent) => {
                let size = item.size.ok_or_else(|| {
                    AppError::BadRequest(format!("choose a size for {}", product.name))
                })?;
                let price = scent_price(scent, size).ok_or_else(|| {
                    AppError::BadRequest(format!(
                        "{} isn't available in {}",
                        product.name,
                        size.label()
                    ))
                })?;
                let unit_cents = money::to_cents(price).ok_or_else(|| {
                    AppError::Internal(format!("scent {} has an invalid price", scent.id))
                })?;
                lines.push(PricedLine {
                    product_id: product.id,
                    name: format!("{} ({})", product.name, size.label()),
                    quantity: item.quantity,
                    unit_cents,
                    scent: Some((scent.id, size, price)),
                });
            }
        }
    }

    let subtotal_cents: i64 = lines
        .iter()
        .map(|l| l.unit_cents * l.quantity as i64)
        .sum();
    let settings = load_settings(&state.db).await?;
    let shipping_cents = shipping_for(&settings, subtotal_cents);

    // Validation is done; nothing is written for a checkout that cannot go
    // ahead. Same placement and reasoning as the share page.
    ensure_public_checkout_live(&state)?;

    let mut tx = state.db.begin().await?;

    // Create-if-absent without touching an existing record — see
    // `public::checkout` for why the no-op update is there.
    let customer = sqlx::query_as::<_, Customer>(
        r#"
        insert into customers (email, name, marketing_consent)
        values ($1, $2, false)
        on conflict (email) do update set email = customers.email
        returning *
        "#,
    )
    .bind(&email)
    .bind(&name)
    .fetch_one(&mut *tx)
    .await?;

    // A coupon is personal: it only counts if it belongs to this buyer.
    let coupon = match body.coupon_code.as_deref() {
        Some(code) if !code.trim().is_empty() => {
            crate::referrals::find_coupon(&state.db, code, Some(customer.id)).await?
        }
        _ => None,
    };
    let coupon_amount = coupon.as_ref().map(|c| c.amount_cents).unwrap_or(0);
    let (total_cents, discount_cents) =
        crate::referrals::apply_discount(subtotal_cents + shipping_cents, coupon_amount);

    let cart = sqlx::query_as::<_, Cart>(
        r#"
        insert into carts (customer_id, currency, total_cents, idempotency_key, note,
                           discount_cents, coupon_id)
        values ($1, $2, $3, $4, $5, $6, $7)
        returning *
        "#,
    )
    .bind(customer.id)
    .bind(DEFAULT_CURRENCY)
    .bind(total_cents)
    .bind(Uuid::new_v4().to_string())
    .bind("Online shop order — ship to the address on the Square order")
    .bind(discount_cents)
    .bind(coupon.as_ref().map(|c| c.id))
    .fetch_one(&mut *tx)
    .await?;

    if let Some(c) = &coupon {
        if !crate::referrals::redeem_coupon(&mut tx, c.id, cart.id).await? {
            tracing::warn!(cart_id = %cart.id, "coupon was spent elsewhere first");
        }
    }

    for line in &lines {
        match line.scent {
            None => {
                sqlx::query(
                    "insert into cart_items \
                       (cart_id, name, quantity, unit_amount_cents, kind, shop_product_id) \
                     values ($1, $2, $3, $4, 'product', $5)",
                )
                .bind(cart.id)
                .bind(&line.name)
                .bind(line.quantity)
                .bind(line.unit_cents)
                .bind(line.product_id)
                .execute(&mut *tx)
                .await?;
            }
            // One order per bottle, because an order is one bottle for staff
            // to make up — and a cart line can claim only one order, so each
            // bottle is its own line too.
            Some((scent_id, size, price)) => {
                for _ in 0..line.quantity {
                    let order = sqlx::query_as::<_, Order>(
                        r#"
                        insert into orders (customer_id, type, size, scent_id, status, amount, external_ref)
                        values ($1, $2, $3, $4, $5, $6, 'public_shop')
                        returning *
                        "#,
                    )
                    .bind(customer.id)
                    .bind(OrderType::SetPerfume)
                    .bind(size)
                    .bind(scent_id)
                    .bind(OrderStatus::Lead)
                    .bind(price)
                    .fetch_one(&mut *tx)
                    .await?;

                    sqlx::query(
                        "insert into cart_items \
                           (cart_id, order_id, name, quantity, unit_amount_cents, kind, shop_product_id) \
                         values ($1, $2, $3, 1, $4, 'blend', $5)",
                    )
                    .bind(cart.id)
                    .bind(order.id)
                    .bind(&line.name)
                    .bind(line.unit_cents)
                    .bind(line.product_id)
                    .execute(&mut *tx)
                    .await?;
                }
            }
        }
    }

    if shipping_cents > 0 {
        sqlx::query(
            "insert into cart_items (cart_id, name, quantity, unit_amount_cents, kind) \
             values ($1, 'Shipping', 1, $2, 'fee')",
        )
        .bind(cart.id)
        .bind(shipping_cents)
        .execute(&mut *tx)
        .await?;
    }

    crate::sync::enqueue(&mut *tx, SyncEntity::Contact, customer.id).await?;

    tx.commit().await?;

    let mut line_items: Vec<LineItemPush> = lines
        .iter()
        .map(|l| LineItemPush {
            name: l.name.clone(),
            quantity: l.quantity,
            unit_amount_cents: l.unit_cents,
        })
        .collect();
    if shipping_cents > 0 {
        line_items.push(LineItemPush {
            name: "Shipping".into(),
            quantity: 1,
            unit_amount_cents: shipping_cents,
        });
    }

    let push = CheckoutPush {
        cart_id: cart.id,
        idempotency_key: cart.idempotency_key.clone(),
        currency: cart.currency.clone(),
        buyer_email: Some(customer.email.clone()),
        line_items,
        discounts: if discount_cents > 0 {
            vec![DiscountPush {
                name: "Coupon".to_string(),
                amount_cents: discount_cents,
            }]
        } else {
            Vec::new()
        },
        redirect_url: Some(format!("{}/thanks?from=shop", site_url())),
        note: cart.note.clone(),
        ask_for_shipping_address: true,
    };

    let handle = match state.square.create_checkout(&push).await {
        Ok(handle) => handle,
        Err(e) => {
            tracing::error!(cart_id = %cart.id, "shop checkout failed: {e}");
            // Nothing was sent to the buyer, so unwind what this request wrote:
            // the coupon goes back, and the made-to-order bottles that were only
            // raised for this cart are removed rather than left as leads on the
            // buyer's record.
            if let Err(e) =
                crate::billing::cancel_cart(&state.db, state.square.as_ref(), cart.id).await
            {
                tracing::error!(cart_id = %cart.id, "could not cancel the failed shop cart: {e}");
            }
            return Err(AppError::Unavailable(
                "could not start checkout — please try again".into(),
            ));
        }
    };

    sqlx::query(
        r#"
        update carts set
            status = 'pending_payment',
            payment_method = 'link',
            square_order_id = $2,
            square_payment_link_id = $3,
            checkout_url = $4,
            checkout_at = now(),
            updated_at = now()
        where id = $1
        "#,
    )
    .bind(cart.id)
    .bind(&handle.square_order_id)
    .bind(&handle.payment_link_id)
    .bind(&handle.url)
    .execute(&state.db)
    .await?;

    tracing::info!(
        cart_id = %cart.id,
        lines = lines.len(),
        total = %money::format_cents(total_cents, DEFAULT_CURRENCY),
        "shop checkout created"
    );

    Ok(Json(ShopCheckoutResponse {
        checkout_url: handle.url,
        subtotal_cents,
        shipping_cents,
        discount_cents,
        total_cents,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_the_three_accepted_formats() {
        assert_eq!(sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_image(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff_image(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some("image/webp"));
    }

    #[test]
    fn rejects_anything_else() {
        assert_eq!(sniff_image(b"<svg xmlns="), None);
        assert_eq!(sniff_image(b"GIF89a"), None);
        assert_eq!(sniff_image(b""), None);
        // RIFF alone is also WAV and AVI.
        assert_eq!(sniff_image(b"RIFF\x00\x00\x00\x00WAVE"), None);
    }

    #[test]
    fn free_shipping_applies_at_the_threshold() {
        let s = ShopSettings {
            shop_shipping_cents: 800,
            shop_free_shipping_over_cents: Some(7500),
        };
        assert_eq!(shipping_for(&s, 7499), 800);
        assert_eq!(shipping_for(&s, 7500), 0);

        let always = ShopSettings {
            shop_shipping_cents: 800,
            shop_free_shipping_over_cents: None,
        };
        assert_eq!(shipping_for(&always, 1_000_000), 800);
    }
}
