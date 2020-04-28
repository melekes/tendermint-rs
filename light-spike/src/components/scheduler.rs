use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::prelude::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationOptions {
    pub trust_threshold: TrustThreshold,
    pub trusting_period: Duration,
    pub now: SystemTime,
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchedulerError {
    #[error("invalid light block")]
    InvalidLightBlock(#[from] VerifierError),
}

impl_event!(SchedulerError);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchedulerInput {
    VerifyUntrustedLightBlock(LightBlock),
}

impl_event!(SchedulerInput);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchedulerOutput {
    ValidLightBlock(Vec<TrustedState>),
}

impl_event!(SchedulerOutput);

pub struct Scheduler {
    trusted_store: TSReader,
}

impl Scheduler {
    pub fn new(trusted_store: TSReader) -> Self {
        Self { trusted_store }
    }

    pub fn verify_light_block(
        &self,
        router: &impl Router,
        trusted_state: TrustedState,
        light_block: LightBlock,
        options: VerificationOptions,
    ) -> Result<SchedulerOutput, SchedulerError> {
        if let Some(trusted_state_in_store) = self.trusted_store.get(light_block.height) {
            return self.verification_succeded(trusted_state_in_store);
        }

        let verifier_result = self.perform_verify_light_block(
            router,
            trusted_state.clone(),
            light_block.clone(),
            options,
        );

        match verifier_result {
            VerifierResponse::VerificationSucceeded(trusted_state) => {
                self.verification_succeded(trusted_state)
            }
            VerifierResponse::VerificationFailed(err) => {
                self.verification_failed(router, err, trusted_state, light_block, options)
            }
        }
    }

    fn perform_verify_light_block(
        &self,
        router: &impl Router,
        trusted_state: TrustedState,
        light_block: LightBlock,
        options: VerificationOptions,
    ) -> VerifierResponse {
        router.query_verifier(VerifierRequest::VerifyLightBlock {
            trusted_state,
            light_block,
            options,
        })
    }

    fn verification_succeded(
        &self,
        new_trusted_state: TrustedState,
    ) -> Result<SchedulerOutput, SchedulerError> {
        Ok(SchedulerOutput::ValidLightBlock(vec![new_trusted_state]))
    }

    fn verification_failed(
        &self,
        router: &impl Router,
        err: VerifierError,
        trusted_state: TrustedState,
        light_block: LightBlock,
        options: VerificationOptions,
    ) -> Result<SchedulerOutput, SchedulerError> {
        match err {
            VerifierError::InvalidLightBlock(VerificationError::InsufficientVotingPower {
                ..
            }) => self.perform_bisection(router, trusted_state, light_block, options),
            err => {
                let output = SchedulerError::InvalidLightBlock(err);
                Err(output)
            }
        }
    }

    fn perform_bisection(
        &self,
        router: &impl Router,
        trusted_state: TrustedState,
        light_block: LightBlock,
        options: VerificationOptions,
    ) -> Result<SchedulerOutput, SchedulerError> {
        // Get the pivot height for bisection.
        let trusted_height = trusted_state.header.height;
        let untrusted_height = light_block.height;
        let pivot_height = trusted_height
            .checked_add(untrusted_height)
            .expect("height overflow")
            / 2;

        let pivot_light_block = self.request_fetch_light_block(router, pivot_height)?;

        let SchedulerOutput::ValidLightBlock(mut pivot_trusted_states) =
            self.verify_light_block(router, trusted_state, pivot_light_block, options)?;

        let trusted_state_left = pivot_trusted_states.last().cloned().unwrap(); // FIXME: Unwrap

        let SchedulerOutput::ValidLightBlock(mut new_trusted_states) =
            self.verify_light_block(router, trusted_state_left, light_block, options)?;

        new_trusted_states.append(&mut pivot_trusted_states);
        new_trusted_states.sort_by_key(|ts| ts.header.height);

        Ok(SchedulerOutput::ValidLightBlock(new_trusted_states))
    }

    fn request_fetch_light_block(
        &self,
        router: &impl Router,
        height: Height,
    ) -> Result<LightBlock, SchedulerError> {
        let rpc_response = router.query_rpc(RpcRequest::FetchLightBlock(height));

        match rpc_response {
            RpcResponse::FetchedLightBlock(light_block) => Ok(light_block),
        }
    }
}