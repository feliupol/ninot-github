//! User and organization profiles.

use std::sync::Mutex;

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::render;
use crate::state::{AccountKind, State, lock};

impl Handler for ops::UsersGetAuthenticated {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_caller()?;
        let state = lock(state);
        let account = state
            .account(&caller.login)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(
            200,
            &render::public_user(request, account),
        ))
    }
}

impl Handler for ops::UsersGetByUsername {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let account = state
            .account(request.param("username"))
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(
            200,
            &render::public_user(request, account),
        ))
    }
}

impl Handler for ops::OrgsGet {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let account = state
            .account(request.param("org"))
            .filter(|account| account.kind == AccountKind::Organization)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(
            200,
            &render::organization(request, account),
        ))
    }
}
