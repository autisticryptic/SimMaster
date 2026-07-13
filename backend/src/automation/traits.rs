use crate::state::AppState;
use anyhow::Result;
use futures_util::future::BoxFuture;

pub trait AutomationTaskHandler: Send + Sync {
    fn task_type(&self) -> &'static str;
    fn execute<'a>(
        &'a self,
        app: &'a AppState,
        params: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<()>>;
}
