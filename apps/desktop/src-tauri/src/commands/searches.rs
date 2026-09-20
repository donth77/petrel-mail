//! Saved searches: the rail's section, and what the middle pane writes into it.
//!
//! Thin by design. A saved search is a name and a query, so these commands are
//! the store's own calls with the account on screen filled in — nothing here
//! decides anything (docs 22).

use crate::state::{AppState, active_account};
use std::sync::Arc;
use tauri::State;

#[tauri::command(async)]
pub fn list_saved_searches(
    state: State<Arc<AppState>>,
) -> Result<Vec<petrel_engine::store::SavedSearch>, String> {
    let store = state.store()?;
    // No account, no searches — the same wall the search itself keeps, and the
    // rail asks for these before onboarding is finished.
    let Some(account) = store.active_account().map_err(|e| e.to_string())? else {
        return Ok(Vec::new());
    };
    store.saved_searches(account).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn create_saved_search(
    name: String,
    query: String,
    state: State<Arc<AppState>>,
) -> Result<i64, String> {
    let mut store = state.store()?;
    let account = active_account(&store)?;
    store
        .create_saved_search(account, &name, &query)
        .map_err(|e| e.to_string())
}

/// Renames one, or updates it to the query now in the field, or both.
#[tauri::command(async)]
pub fn update_saved_search(
    id: i64,
    name: Option<String>,
    query: Option<String>,
    state: State<Arc<AppState>>,
) -> Result<(), String> {
    let mut store = state.store()?;
    store
        .update_saved_search(id, name.as_deref(), query.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn delete_saved_search(id: i64, state: State<Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.delete_saved_search(id).map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub fn move_saved_search(id: i64, up: bool, state: State<Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store.move_saved_search(id, up).map_err(|e| e.to_string())
}

/// The rail's order, as dragged.
#[tauri::command(async)]
pub fn reorder_saved_searches(ids: Vec<i64>, state: State<Arc<AppState>>) -> Result<(), String> {
    let mut store = state.store()?;
    store
        .reorder_saved_searches(&ids)
        .map_err(|e| e.to_string())
}
