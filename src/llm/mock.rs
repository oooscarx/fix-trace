use std::{collections::VecDeque, sync::Mutex};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::{
    error::AppError,
    llm::provider::{LlmProvider, LlmRequest, LlmResponse},
};

pub struct MockProvider {
    responses: Mutex<VecDeque<LlmResponse>>,
}

impl MockProvider {
    pub fn new(responses: impl IntoIterator<Item = LlmResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
        }
    }
}

#[async_trait]
impl LlmProvider for MockProvider {
    async fn complete(
        &self,
        _request: LlmRequest,
        cancellation: CancellationToken,
    ) -> Result<LlmResponse, AppError> {
        if cancellation.is_cancelled() {
            return Err(AppError::Agent("mock request cancelled".to_owned()));
        }
        self.responses
            .lock()
            .map_err(|_| AppError::Llm("mock response queue was poisoned".to_owned()))?
            .pop_front()
            .ok_or_else(|| AppError::Llm("mock response queue is empty".to_owned()))
    }
}
