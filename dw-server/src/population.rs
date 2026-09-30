use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use bitdemon::lobby::matchmaking::SessionRegistry;
use bitdemon::lobby::messaging::MessageRouter;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone)]
struct PopulationState {
    sessions: Arc<SessionRegistry>,
    router: Arc<MessageRouter>,
}

#[derive(Serialize)]
struct PopulationResponse {
    online: usize,
    playing: usize,
    playlists: BTreeMap<u32, usize>,
}

pub fn router(sessions: Arc<SessionRegistry>, router: Arc<MessageRouter>) -> Router {
    Router::new()
        .route("/v1/population", get(population_totals))
        .with_state(PopulationState { sessions, router })
}

async fn population_totals(State(state): State<PopulationState>) -> Json<PopulationResponse> {
    let population = state.sessions.population();

    Json(PopulationResponse {
        online: state.router.online(),
        playing: population.players,
        playlists: population.playlists,
    })
}
