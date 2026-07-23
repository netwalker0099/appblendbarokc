use axum::extract::{Path, State};
use axum::Json;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::models::ingredient::{Ingredient, IngredientType};
use crate::models::mix::MAX_MIX_INGREDIENTS;
use crate::AppState;

pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Ingredient>>, AppError> {
    let ingredients =
        sqlx::query_as::<_, Ingredient>("select * from ingredients order by name")
            .fetch_all(&state.db)
            .await?;
    Ok(Json(ingredients))
}

#[derive(Deserialize)]
pub struct CreateIngredientRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub ingredient_type: IngredientType,
}

pub async fn create(
    State(state): State<AppState>,
    Json(body): Json<CreateIngredientRequest>,
) -> Result<Json<Ingredient>, AppError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }

    let ingredient = sqlx::query_as::<_, Ingredient>(
        "insert into ingredients (name, type) values ($1, $2) returning *",
    )
    .bind(name)
    .bind(body.ingredient_type)
    .fetch_one(&state.db)
    .await
            .map_err(|e| match &e {
                sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                    AppError::Conflict("an ingredient with this name already exists".into())
                }
                _ => AppError::from(e),
            })?;

    Ok(Json(ingredient))
}

#[derive(Deserialize)]
pub struct UpdateIngredientRequest {
    pub name: Option<String>,
    pub active: Option<bool>,
    #[serde(rename = "type")]
    pub ingredient_type: Option<IngredientType>,
}

pub async fn update(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<UpdateIngredientRequest>,
) -> Result<Json<Ingredient>, AppError> {
    let ingredient = sqlx::query_as::<_, Ingredient>(
        "update ingredients set name = coalesce($2, name), active = coalesce($3, active), type = coalesce($4, type) where id = $1 returning *",
    )
    .bind(id)
    .bind(body.name)
    .bind(body.active)
    .bind(body.ingredient_type)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("ingredient not found".into()))?;

    Ok(Json(ingredient))
}

/// Validates that `ids` is a non-empty, deduplicated set of at most
/// [`MAX_MIX_INGREDIENTS`] active ingredients. Used by both the mix CRUD
/// handler and `/api/intake` so the rule can't drift between the two paths.
pub async fn assert_active_ingredients(pool: &PgPool, ids: &[Uuid]) -> Result<(), AppError> {
    if ids.is_empty() {
        return Err(AppError::BadRequest(
            "a mix needs at least 1 ingredient".into(),
        ));
    }
    if ids.len() > MAX_MIX_INGREDIENTS {
        return Err(AppError::BadRequest(format!(
            "a mix may have at most {MAX_MIX_INGREDIENTS} ingredients"
        )));
    }

    let unique: std::collections::HashSet<&Uuid> = ids.iter().collect();
    if unique.len() != ids.len() {
        return Err(AppError::BadRequest(
            "a mix cannot repeat the same ingredient twice".into(),
        ));
    }

    let count: i64 = sqlx::query_scalar(
        "select count(*) from ingredients where id = any($1) and active = true",
    )
    .bind(ids)
    .fetch_one(pool)
    .await?;

    if count as usize != ids.len() {
        return Err(AppError::BadRequest(
            "one or more ingredients are invalid or inactive".into(),
        ));
    }

    Ok(())
}
