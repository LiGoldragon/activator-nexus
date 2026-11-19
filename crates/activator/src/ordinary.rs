//! The ordinary Signal, answered only by the activator's target-local effects.

use signal_activator_nexus::{Query, Response};

use crate::activation::{Activatable as _, Activator};

pub trait Answerable {
    fn answer(&self, query: Query) -> impl Future<Output = Response> + Send;
}

impl Answerable for Activator {
    async fn answer(&self, query: Query) -> Response {
        match query {
            Query::Activate(generation) => match self.activate(generation).await {
                Ok(generation) => Response::Activated(generation),
                Err(refusal) => Response::ActivateRefused(refusal),
            },
            Query::Live => match self.live().await {
                Ok(generation) => Response::Live(generation),
                Err(refusal) => Response::LiveRefused(refusal),
            },
        }
    }
}
