use actix_web::web;

mod files;
mod health;
mod helpers;
mod issues;
mod logs;
mod uploads;

pub fn register(cfg: &mut web::ServiceConfig) {
    cfg.service(health::health).service(
        web::scope("/api")
            .service(issues::list_issues)
            .service(issues::get_issue_bundles)
            .service(issues::delete_issue_bundle)
            .service(issues::delete_issue)
            .service(files::get_file_node)
            .service(files::get_file_content)
            .service(files::delete_file_node)
            .service(logs::search_issue_logs)
            .service(logs::search_logs)
            .service(uploads::upload_logs),
    );
}
