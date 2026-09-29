use crate::state::AppState;
use anyhow::Result;
use futures_util::future::BoxFuture;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationExecutionReport {
    Completed,
    Dial(super::tasks::dial_outcome::DialReport),
}

impl AutomationExecutionReport {
    pub fn detail(&self) -> String {
        match self {
            Self::Completed => "执行成功".into(),
            Self::Dial(report) => report.detail(),
        }
    }
}

pub trait AutomationTaskHandler: Send + Sync {
    /// Additive report seam; existing task handlers retain their Result<()> API.
    fn execute_report<'a>(
        &'a self,
        app: &'a AppState,
        params: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<AutomationExecutionReport>> {
        Box::pin(async move {
            self.execute(app, params).await?;
            Ok(AutomationExecutionReport::Completed)
        })
    }
    fn task_type(&self) -> &'static str;
    fn execute<'a>(
        &'a self,
        app: &'a AppState,
        params: &'a serde_json::Value,
    ) -> BoxFuture<'a, Result<()>>;
}
