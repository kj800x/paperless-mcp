mod mcp;
mod metrics;
mod paperless;
mod tools;

use actix_web::{App, HttpResponse, HttpServer, Responder, get, web};

#[get("/healthz")]
async fn healthz() -> impl Responder {
    HttpResponse::Ok().body("ok")
}

#[get("/metrics")]
async fn metrics_endpoint(metrics: web::Data<metrics::Metrics>) -> impl Responder {
    HttpResponse::Ok()
        .content_type("text/plain; version=0.0.4")
        .body(metrics.render())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .parse_default_env()
        .init();

    let base_url = std::env::var("PAPERLESS_URL")
        .unwrap_or("http://webserver.paperless.svc.cluster.local:8000".to_string());
    let public_url = std::env::var("PAPERLESS_PUBLIC_URL")
        .unwrap_or("https://paperless.home.coolkev.com".to_string());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);

    log::info!(
        "Proxying to Paperless at {}; listening on :{}",
        base_url,
        port
    );

    let paperless = web::Data::new(paperless::Paperless::new(base_url, public_url));
    let metrics = web::Data::new(metrics::Metrics::new());

    HttpServer::new(move || {
        App::new()
            .app_data(paperless.clone())
            .app_data(metrics.clone())
            .app_data(web::JsonConfig::default().limit(1 << 20))
            .service(healthz)
            .service(metrics_endpoint)
            .route("/mcp", web::post().to(mcp::handle_mcp))
            // No server-initiated streams; per the Streamable HTTP spec, GET may 405.
            .route("/mcp", web::get().to(HttpResponse::MethodNotAllowed))
    })
    .bind(("0.0.0.0", port))?
    .run()
    .await
}
