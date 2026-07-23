use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Perfumery classification of an ingredient. Every ingredient is one of these,
/// and the mix/scent builders group the picker by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum IngredientType {
    Base,
    TopNote,
    HeartNote,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Ingredient {
    pub id: Uuid,
    pub name: String,
    #[sqlx(rename = "type")]
    #[serde(rename = "type")]
    pub ingredient_type: IngredientType,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}
