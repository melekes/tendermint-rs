use tendermint::{block, rpc};

use crate::prelude::*;
use std::future::Future;

pub enum RequesterEvent {
    // Inputs
    FetchSignedHeader(Height),
    FetchValidatorSet(Height),
    FetchState(Height),
    // Outputs
    SignedHeader(Height, SignedHeader),
    ValidatorSet(Height, ValidatorSet),
    FetchedState {
        height: Height,
        signed_header: SignedHeader,
        validator_set: ValidatorSet,
        next_validator_set: ValidatorSet,
    },
}

pub struct Requester {
    rpc_client: rpc::Client,
}

impl Requester {
    pub fn new(rpc_client: rpc::Client) -> Self {
        Self { rpc_client }
    }

    pub fn fetch_signed_header(&self, h: Height) -> Result<SignedHeader, Error> {
        let height: block::Height = h.into();

        let res = block_on(async {
            match height.value() {
                0 => self.rpc_client.latest_commit().await,
                _ => self.rpc_client.commit(height).await,
            }
        });

        match res {
            Ok(response) => Ok(response.signed_header.into()),
            Err(error) => Err(todo!()),
        }
    }

    pub fn fetch_validator_set(&self, h: Height) -> Result<ValidatorSet, Error> {
        let height: block::Height = h.into();

        let res = block_on(self.rpc_client.validators(h));

        match res {
            Ok(response) => Ok(response.validators.into()),
            Err(error) => Err(todo!()),
        }
    }
}

impl Handler<RequesterEvent> for Requester {
    fn handle(&mut self, event: RequesterEvent) -> Event {
        use RequesterEvent::*;

        match event {
            FetchSignedHeader(height) => {
                let signed_header = self.fetch_signed_header(height);
                match signed_header {
                    Ok(signed_header) => RequesterEvent::SignedHeader(height, signed_header).into(),
                    Err(err) => todo!(),
                }
            }
            FetchValidatorSet(height) => {
                let validator_set = self.fetch_validator_set(height);
                match validator_set {
                    Ok(validator_set) => RequesterEvent::ValidatorSet(height, validator_set).into(),
                    Err(err) => todo!(),
                }
            }
            FetchState(height) => {
                let signed_header = self.fetch_signed_header(height);
                let validator_set = self.fetch_validator_set(height);
                let next_validator_set = self.fetch_validator_set(height + 1);

                match (signed_header, validator_set, next_validator_set) {
                    (Ok(signed_header), Ok(validator_set), Ok(next_validator_set)) => {
                        RequesterEvent::FetchedState {
                            height,
                            signed_header,
                            validator_set,
                            next_validator_set,
                        }
                        .into()
                    }
                    _ => todo!(),
                }
            }
            _ => unreachable!(),
        }
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new()
        .basic_scheduler()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}