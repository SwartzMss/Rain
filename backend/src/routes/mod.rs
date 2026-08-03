use actix_web::{
    Error,
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    http::header::{CACHE_CONTROL, HeaderValue},
    middleware::{Next, from_fn},
    web,
};

mod auth;
mod files;
mod health;
mod helpers;
mod issues;
#[cfg(test)]
pub(crate) use issues::cleanup_inactive_issues;
pub use issues::resume_manual_issue_deletions;
mod logs;
mod saved_searches;
mod temp_results;
mod uploads;

pub fn spawn_temp_result_cleanup(state: web::Data<crate::AppState>) -> tokio::task::JoinHandle<()> {
    crate::spawn_periodic_job(
        "temporary-result-cleanup",
        std::time::Duration::from_secs(60),
        std::time::Duration::from_secs(300),
        move || {
            let state = state.clone();
            async move {
                temp_results::cleanup_expired(&state)
                    .await
                    .map_err(|error| error.to_string())
            }
        },
    )
}

pub fn spawn_inactive_issue_cleanup(
    state: web::Data<crate::AppState>,
) -> tokio::task::JoinHandle<()> {
    crate::spawn_periodic_job(
        "inactive-issue-cleanup",
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(60 * 60),
        move || {
            let state = state.clone();
            async move {
                issues::cleanup_inactive_issues(&state)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }
        },
    )
}

pub fn spawn_manual_issue_cleanup(
    state: web::Data<crate::AppState>,
) -> tokio::task::JoinHandle<()> {
    crate::spawn_periodic_job(
        "manual-issue-cleanup",
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(60),
        move || {
            let pool = state.db.pool.clone();
            async move {
                issues::resume_manual_issue_deletions(&pool)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            }
        },
    )
}

async fn prevent_session_response_caching(
    request: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    let no_store =
        request.path().starts_with("/api/auth/") || request.path().starts_with("/api/me/");
    let mut response = next.call(request).await?;
    if no_store {
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store, private"));
    }
    Ok(response)
}

pub fn register(cfg: &mut web::ServiceConfig) {
    cfg.service(health::health)
        .service(health::readiness)
        .service(
            web::scope("/api")
                .wrap(from_fn(prevent_session_response_caching))
                .service(auth::register_user)
                .service(auth::registration_status)
                .service(admin::list_users)
                .service(admin::get_settings)
                .service(admin::update_settings)
                .service(admin::auth_rate_limits)
                .service(admin::clear_username_rate_limit)
                .service(admin::clear_ip_rate_limit)
                .service(admin::clear_username_rate_limits)
                .service(admin::clear_ip_rate_limits)
                .service(admin::change_status)
                .service(admin::revoke_sessions)
                .service(admin::list_audit)
                .service(auth::login)
                .service(auth::me)
                .service(auth::logout)
                .service(auth::change_password)
                .service(saved_searches::list)
                .service(saved_searches::create)
                .service(saved_searches::update)
                .service(saved_searches::delete)
                .service(saved_searches::mark_used)
                .service(issues::list_issues)
                .service(issues::create_issue)
                .service(issues::get_issue_bundles)
                .service(issues::delete_issue_bundle)
                .service(issues::delete_issue)
                .service(files::get_file_node)
                .service(files::get_file_content)
                .service(files::get_file_lines)
                .service(files::download_file)
                .service(files::delete_file_node)
                .service(logs::search_issue_logs)
                .service(logs::search_logs)
                .service(temp_results::create_temp_result)
                .service(temp_results::preview_temp_result)
                .service(temp_results::get_temp_result)
                .service(temp_results::get_temp_result_lines)
                .service(temp_results::download_temp_result)
                .service(temp_results::delete_temp_result)
                .service(uploads::upload_logs)
                .service(uploads::get_upload_task),
        );
}
mod admin;
