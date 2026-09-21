use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use bitdemon::lobby::matchmaking::SessionRegistry;
use bitdemon::networking::session_manager::SessionManager;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone)]
struct PopulationState {
    sessions: Arc<SessionRegistry>,
    online: Arc<AtomicUsize>,
}

#[derive(Serialize)]
struct PopulationResponse {
    online: usize,
    playing: usize,
    playlists: BTreeMap<u32, usize>,
}

pub fn router(sessions: Arc<SessionRegistry>, session_manager: &SessionManager) -> Router {
    let online = Arc::new(AtomicUsize::new(0));

    {
        let online = online.clone();
        session_manager.on_session_registered(move |_| {
            online.fetch_add(1, Ordering::Relaxed);
        });
    }

    {
        let online = online.clone();
        session_manager.on_session_unregistered(move |_| {
            let _ = online.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
        });
    }

    Router::new()
        .route("/v1/population", get(population_totals))
        .with_state(PopulationState { sessions, online })
}

async fn population_totals(State(state): State<PopulationState>) -> Json<PopulationResponse> {
    let population = state.sessions.population();

    Json(PopulationResponse {
        online: state.online.load(Ordering::Relaxed),
        playing: population.players,
        playlists: population.playlists,
    })
}
