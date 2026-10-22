//! Saved workflows.

use axum::Json;
use axum::extract::State;
use so_workflow::{SavedSource, SavedWorkflows};

use crate::error::ApiResult;
use crate::state::AppState;
use crate::types::WorkflowSummary;

pub async fn list(State(st): State<AppState>) -> ApiResult<Json<Vec<WorkflowSummary>>> {
    st.clone()
        .blocking(move || {
            let saved = SavedWorkflows::new(st.sup.paths.workflows_dir(), st.project_cwd()).list();
            Ok(Json(
                saved
                    .into_iter()
                    .map(|w| WorkflowSummary {
                        name: w.name,
                        path: w.path.display().to_string(),
                        source: match w.source {
                            SavedSource::Project => "project",
                            SavedSource::Config => "config",
                        }
                        .to_string(),
                    })
                    .collect(),
            ))
        })
        .await
}
