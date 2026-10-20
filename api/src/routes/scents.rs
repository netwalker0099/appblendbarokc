use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::Json;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::employee_auth::AdminEmployee;
use crate::error::AppError;
use crate::models::mix::check_formula_total;
use crate::models::scent::{Scent, ScentItem};
use crate::routes::ingredients::assert_active_ingredients;
use crate::routes::mixes::MixItemInput;
use crate::AppState;

#[derive(Serialize)]
pub struct ScentDetail {
    #[serde(flatten)]
    pub scent: Scent,
    pub items: Vec<ScentItem>,
}

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<ScentDetail>>, AppError> {
    let scents = sqlx::query_as::<_, Scent>("select * from scents order by name")
        .fetch_all(&state.db)
        .await?;

    // One query for every scent's items, then bucket — no per-scent round trip.
    let scent_ids: Vec<Uuid> = scents.iter().map(|s| s.id).collect();
    let items = sqlx::query_as::<_, ScentItem>("select * from scent_items where scent_id = any($1)")
        .bind(&scent_ids)
        .fetch_all(&state.db)
        .await?;

    let mut by_scent: HashMap<Uuid, Vec<ScentItem>> = HashMap::new();
    for item in items {
        by_scent.entry(item.scent_id).or_default().push(item);
    }
    let details = scents
        .into_iter()
        .map(|scent| {
            let items = by_scent.remove(&scent.id).unwrap_or_default();
            ScentDetail { scent, items }
        })
        .collect();

    Ok(Json(details))
}

pub async fn get(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<ScentDetail>, AppError> {
    let detail = fetch_scent_detail(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("scent not found".into()))?;
    Ok(Json(detail))
}

pub async fn fetch_scent_detail(pool: &PgPool, id: Uuid) -> Result<Option<ScentDetail>, AppError> {
    let Some(scent) = sqlx::query_as::<_, Scent>("select * from scents where id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(None);
    };
    let items = sqlx::query_as::<_, ScentItem>("select * from scent_items where scent_id = $1")
        .bind(id)
        .fetch_all(pool)
        .await?;
    Ok(Some(ScentDetail { scent, items }))
}

/// Validates a scent's formula: an empty formula is allowed (recipe not defined
/// yet); a non-empty one must be at most 8 distinct active ingredients with
/// positive amounts totalling exactly 100 ml — the same rule mixes use.
async fn validate_formula(pool: &PgPool, items: &[MixItemInput]) -> Result<(), AppError> {
    if items.is_empty() {
        return Ok(());
    }
    let ids: Vec<Uuid> = items.iter().map(|i| i.ingredient_id).collect();
    assert_active_ingredients(pool, &ids).await?;
    for item in items {
        if item.amount_ml <= Decimal::ZERO {
            return Err(AppError::BadRequest("amount_ml must be positive".into()));
        }
    }
    check_formula_total(items.iter().map(|i| i.amount_ml))?;
    Ok(())
}

async fn replace_items(
    tx: &mut sqlx::PgConnection,
    scent_id: Uuid,
    items: &[MixItemInput],
) -> Result<(), AppError> {
    sqlx::query("delete from scent_items where scent_id = $1")
        .bind(scent_id)
        .execute(&mut *tx)
        .await?;
    for item in items {
        sqlx::query(
            "insert into scent_items (scent_id, ingredient_id, amount_ml) values ($1, $2, $3)",
        )
        .bind(scent_id)
        .bind(item.ingredient_id)
        .bind(item.amount_ml)
        .execute(&mut *tx)
        .await?;
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct CreateScentRequest {
    pub name: String,
    pub items: Option<Vec<MixItemInput>>,
}

pub async fn create(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Json(body): Json<CreateScentRequest>,
) -> Result<Json<ScentDetail>, AppError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }
    let items = body.items.unwrap_or_default();
    validate_formula(&state.db, &items).await?;

    let mut tx = state.db.begin().await?;
    // A scent with no formula can't be sold at intake, so it starts switched
    // off; Catalog switches it on once the formula is in.
    let scent = sqlx::query_as::<_, Scent>(
        "insert into scents (name, active) values ($1, $2) returning *",
    )
        .bind(name)
        .bind(!items.is_empty())
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                AppError::Conflict("a scent with this name already exists".into())
            }
            _ => AppError::from(e),
        })?;
    replace_items(&mut tx, scent.id, &items).await?;
    tx.commit().await?;

    let detail = fetch_scent_detail(&state.db, scent.id)
        .await?
        .ok_or_else(|| AppError::Internal("scent vanished after create".into()))?;
    Ok(Json(detail))
}

#[derive(Deserialize)]
pub struct ScentPrices {
    pub vintage: Option<Decimal>,
    pub oz3_4: Option<Decimal>,
    pub oz1_7: Option<Decimal>,
    pub oz1_2: Option<Decimal>,
    pub roller: Option<Decimal>,
    pub spray: Option<Decimal>,
}

#[derive(Deserialize)]
pub struct UpdateScentRequest {
    pub name: Option<String>,
    pub active: Option<bool>,
    pub items: Option<Vec<MixItemInput>>,
    // When present, every per-size price is set (a null clears one).
    pub prices: Option<ScentPrices>,
}

pub async fn update(
    _admin: AdminEmployee,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateScentRequest>,
) -> Result<Json<ScentDetail>, AppError> {
    if let Some(items) = &body.items {
        validate_formula(&state.db, items).await?;
    }

    let (set_prices, vintage, p3_4, p1_7, p1_2, roller, spray) = match &body.prices {
        Some(p) => (true, p.vintage, p.oz3_4, p.oz1_7, p.oz1_2, p.roller, p.spray),
        None => (false, None, None, None, None, None, None),
    };
    for price in [vintage, p3_4, p1_7, p1_2, roller, spray]
        .into_iter()
        .flatten()
    {
        if price < Decimal::ZERO {
            return Err(AppError::BadRequest("prices can't be negative".into()));
        }
    }

    let mut tx = state.db.begin().await?;

    // An active scent must have a formula: switching one on without a formula,
    // or clearing the formula of one that is on, is refused. A scent that was
    // already on without one (from before this rule) can still have its name
    // and prices edited.
    let was_active: Option<bool> = sqlx::query_scalar("select active from scents where id = $1")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
    let was_active = was_active.ok_or_else(|| AppError::NotFound("scent not found".into()))?;
    let will_be_active = body.active.unwrap_or(was_active);
    if will_be_active {
        let formula_after = match &body.items {
            Some(items) => items.len() as i64,
            None => sqlx::query_scalar("select count(*) from scent_items where scent_id = $1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await?,
        };
        let switching_on = !was_active;
        let clearing = body.items.as_ref().is_some_and(|i| i.is_empty());
        if formula_after == 0 && (switching_on || clearing) {
            return Err(AppError::BadRequest(
                "an active scent needs a formula totalling 100 ml — add it before \
                 switching the scent on (or switch it off to remove the formula)"
                    .into(),
            ));
        }
    }

    sqlx::query_as::<_, Scent>(
        r#"
        update scents set
          name = coalesce($2, name),
          active = coalesce($3, active),
          price_vintage = case when $4 then $5  else price_vintage end,
          price_oz3_4   = case when $4 then $6  else price_oz3_4 end,
          price_oz1_7   = case when $4 then $7  else price_oz1_7 end,
          price_oz1_2   = case when $4 then $8  else price_oz1_2 end,
          price_roller  = case when $4 then $9  else price_roller end,
          price_spray   = case when $4 then $10 else price_spray end
        where id = $1 returning *
        "#,
    )
    .bind(id)
    .bind(&body.name)
    .bind(body.active)
    .bind(set_prices)
    .bind(vintage)
    .bind(p3_4)
    .bind(p1_7)
    .bind(p1_2)
    .bind(roller)
    .bind(spray)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::NotFound("scent not found".into()))?;

    if let Some(items) = &body.items {
        replace_items(&mut tx, id, items).await?;
    }
    tx.commit().await?;

    let detail = fetch_scent_detail(&state.db, id)
        .await?
        .ok_or_else(|| AppError::Internal("scent vanished after update".into()))?;
    Ok(Json(detail))
}

/// Used by the intake handler to validate a `set_perfume` order's `scent_id`.
/// A house scent intake can sell: active, with a formula that totals 100 ml at
/// the base — so every bottle size pours exactly (50 ml for 1.7 oz, 30 ml for
/// 1 oz, 10 ml for a roller or spray). Staff pour a set perfume from this
/// formula; without one there is nothing to measure.
pub async fn assert_sellable_scent(pool: &PgPool, id: Uuid) -> Result<(), AppError> {
    assert_active_scent(pool, id).await?;
    let name: String = sqlx::query_scalar("select name from scents where id = $1")
        .bind(id)
        .fetch_one(pool)
        .await?;
    let amounts: Vec<Decimal> =
        sqlx::query_scalar("select amount_ml from scent_items where scent_id = $1")
            .bind(id)
            .fetch_all(pool)
            .await?;
    if amounts.is_empty() {
        return Err(AppError::BadRequest(format!(
            "\"{name}\" has no formula yet — add one totalling 100 ml in Admin → Catalog \
             before selling it"
        )));
    }
    check_formula_total(amounts).map_err(|_| {
        AppError::BadRequest(format!(
            "\"{name}\"'s formula doesn't total 100 ml — fix it in Admin → Catalog before \
             selling it, or every bottle would be over- or under-filled"
        ))
    })
}

pub async fn assert_active_scent(pool: &PgPool, id: Uuid) -> Result<(), AppError> {
    let exists: bool =
        sqlx::query_scalar("select exists(select 1 from scents where id = $1 and active = true)")
            .bind(id)
            .fetch_one(pool)
            .await?;

    if !exists {
        return Err(AppError::BadRequest(
            "scent_id is invalid or inactive".into(),
        ));
    }

    Ok(())
}
