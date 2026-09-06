use crate::use_cases::{
    AdminGetUserUseCase, AuthenticateAccessTokenUseCase, AuthenticateUserUseCase,
    ChangeUserRoleUseCase, ChangeUserTierUseCase, CheckUserAdminUseCase, CreateAccessTokenUseCase,
    CreateUserUseCase, DeleteAccessTokenUseCase, DeleteAccessTokensUseCase, DeleteUserUseCase,
    FindUserByStripeCustomerIdUseCase, GetAccessTokenUseCase, GetOwnUserUseCase,
    ListAccessTokensUseCase, SearchUsersUseCase, SetUserStripeCustomerIdUseCase,
    SuspendUserUseCase, UpdateAccessTokenUseCase, UpdateUserProfileUseCase,
};
use std::sync::Arc;

pub struct UserUseCases {
    pub create: Arc<dyn CreateUserUseCase>,
    pub update_profile: Arc<dyn UpdateUserProfileUseCase>,
    pub change_role: Arc<dyn ChangeUserRoleUseCase>,
    pub change_tier: Arc<dyn ChangeUserTierUseCase>,
    pub suspend: Arc<dyn SuspendUserUseCase>,
    pub set_stripe_customer_id: Arc<dyn SetUserStripeCustomerIdUseCase>,
    pub get_own: Arc<dyn GetOwnUserUseCase>,
    pub admin_get: Arc<dyn AdminGetUserUseCase>,
    pub check_admin: Arc<dyn CheckUserAdminUseCase>,
    pub search: Arc<dyn SearchUsersUseCase>,
    pub find_by_stripe_customer_id: Arc<dyn FindUserByStripeCustomerIdUseCase>,
    pub create_access_token: Arc<dyn CreateAccessTokenUseCase>,
    pub update_access_token: Arc<dyn UpdateAccessTokenUseCase>,
    pub delete_access_token: Arc<dyn DeleteAccessTokenUseCase>,
    pub delete_access_tokens: Arc<dyn DeleteAccessTokensUseCase>,
    pub delete: Arc<dyn DeleteUserUseCase>,
    pub get_access_token: Arc<dyn GetAccessTokenUseCase>,
    pub list_access_tokens: Arc<dyn ListAccessTokensUseCase>,
    pub authenticate_access_token: Arc<dyn AuthenticateAccessTokenUseCase>,
    pub authenticate_user: Arc<dyn AuthenticateUserUseCase>,
}

pub struct UserUseCasesInput {
    pub create: Arc<dyn CreateUserUseCase>,
    pub update_profile: Arc<dyn UpdateUserProfileUseCase>,
    pub change_role: Arc<dyn ChangeUserRoleUseCase>,
    pub change_tier: Arc<dyn ChangeUserTierUseCase>,
    pub suspend: Arc<dyn SuspendUserUseCase>,
    pub set_stripe_customer_id: Arc<dyn SetUserStripeCustomerIdUseCase>,
    pub get_own: Arc<dyn GetOwnUserUseCase>,
    pub admin_get: Arc<dyn AdminGetUserUseCase>,
    pub check_admin: Arc<dyn CheckUserAdminUseCase>,
    pub search: Arc<dyn SearchUsersUseCase>,
    pub find_by_stripe_customer_id: Arc<dyn FindUserByStripeCustomerIdUseCase>,
    pub create_access_token: Arc<dyn CreateAccessTokenUseCase>,
    pub update_access_token: Arc<dyn UpdateAccessTokenUseCase>,
    pub delete_access_token: Arc<dyn DeleteAccessTokenUseCase>,
    pub delete_access_tokens: Arc<dyn DeleteAccessTokensUseCase>,
    pub delete: Arc<dyn DeleteUserUseCase>,
    pub get_access_token: Arc<dyn GetAccessTokenUseCase>,
    pub list_access_tokens: Arc<dyn ListAccessTokensUseCase>,
    pub authenticate_access_token: Arc<dyn AuthenticateAccessTokenUseCase>,
    pub authenticate_user: Arc<dyn AuthenticateUserUseCase>,
}

impl UserUseCases {
    pub fn new(input: UserUseCasesInput) -> Self {
        Self {
            create: input.create,
            update_profile: input.update_profile,
            change_role: input.change_role,
            change_tier: input.change_tier,
            suspend: input.suspend,
            set_stripe_customer_id: input.set_stripe_customer_id,
            get_own: input.get_own,
            admin_get: input.admin_get,
            check_admin: input.check_admin,
            search: input.search,
            find_by_stripe_customer_id: input.find_by_stripe_customer_id,
            create_access_token: input.create_access_token,
            update_access_token: input.update_access_token,
            delete_access_token: input.delete_access_token,
            delete_access_tokens: input.delete_access_tokens,
            delete: input.delete,
            get_access_token: input.get_access_token,
            list_access_tokens: input.list_access_tokens,
            authenticate_access_token: input.authenticate_access_token,
            authenticate_user: input.authenticate_user,
        }
    }
}
