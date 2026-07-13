use actix_web::web;

mod files;
mod health;
mod helpers;
mod issues;
mod logs;
mod temp_results;
mod uploads;

pub fn register(cfg: &mut web::ServiceConfig) {
    cfg.service(health::health).service(
        web::scope("/api")
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
