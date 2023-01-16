use actix_cors::Cors;
use actix_web::{App, HttpServer, ResponseError};
use actix_web_prom::PrometheusMetricsBuilder;
use actix_web_validator::PathConfig;
// use opentelemetry::{
//     global, runtime::TokioCurrentThread, sdk::propagation::TraceContextPropagator,
// };
use paperclip::actix::{web, OpenApiExt};
pub(crate) use sqlx::types::BigDecimal;
use tracing_actix_web::TracingLogger;
// use tracing_bunyan_formatter::{BunyanFormattingLayer, JsonStorageLayer};
use tracing_subscriber::layer::SubscriberExt;
// use tracing_subscriber::Registry;
mod config;
mod db_helpers;
mod errors;
mod modules;
mod rpc_helpers;
mod types;
use tracing_stackdriver::CloudTraceConfiguration;
// use tracing_subscriber::{layer::SubscriberExt, Registry};
pub(crate) const LOGGER_MSG: &str = "near_enhanced_api";

pub(crate) const MIN_EVENT_INDEX: u128 = (10_u128).pow(34);

pub(crate) type Result<T> = std::result::Result<T, errors::Error>;

fn get_cors(cors_allowed_origins: &[String]) -> Cors {
    let mut cors = Cors::permissive();
    if cors_allowed_origins != ["*".to_string()] {
        for origin in cors_allowed_origins {
            cors = cors.allowed_origin(origin);
        }
    }
    cors.allowed_methods(vec!["GET"])
        .allowed_headers(vec![
            actix_web::http::header::AUTHORIZATION,
            actix_web::http::header::ACCEPT,
            actix_web::http::header::CONTENT_TYPE,
        ])
        .allowed_header("x-api-key")
        .max_age(3600)
}

fn get_api_base_path() -> String {
    std::env::var("API_BASE_PATH").unwrap_or_else(|_| "".to_string())
}

async fn playground_ui() -> impl actix_web::Responder {
    let base_path = get_api_base_path();
    actix_web::HttpResponse::Ok()
        .insert_header(actix_web::http::header::ContentType::html())
        .body(
            format!(r#"<!doctype html>
                <html lang="en">
                  <head>
                    <meta charset="utf-8">
                    <meta name="viewport" content="width=device-width, initial-scale=1, shrink-to-fit=no">
                    <title>NEAR Enhanced API powered by Pagoda - Playground</title>
                    <!-- Embed elements Elements via Web Component -->
                    <script src="https://unpkg.com/@stoplight/elements/web-components.min.js"></script>
                    <link rel="stylesheet" href="https://unpkg.com/@stoplight/elements/styles.min.css">
                  </head>
                  <body>

                    <elements-api
                      apiDescriptionUrl="{base_path}/spec/v3.json"
                      router="hash"
                      layout="sidebar"
                    />

                  </body>
                </html>"#),
        )
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    dotenv::dotenv().ok();

    init_telemetry();

    // set up the root span to trigger Span/Trace ID generation
    let root = tracing::info_span!("root");
    let _root = root.enter();
    tracing::info!("Application starting");
    tracing::debug!(
        target: crate::LOGGER_MSG,
        "NEAR Enhanced API Server is initializing..."
    );

    let prometheus = PrometheusMetricsBuilder::new("api")
        .endpoint("/metrics")
        .build()
        .unwrap();

    // See https://docs.rs/sqlx/latest/sqlx/struct.Pool.html#2-connection-limits-mysql-mssql-postgres
    // for setting connection limits.
    let db_max_connections: u32 = std::env::var("DATABASE_MAX_CONNECTIONS")
        .unwrap_or_else(|_| "97".to_string())
        .parse()
        .expect("Failed to parse DATABASE_MAX_CONNECTIONS value as u32");

    let explorer_db_url = &std::env::var("EXPLORER_DATABASE_URL")
        .expect("failed to get database url from EXPLORER_DATABASE_URL env variable");
    let pool_explorer = sqlx::postgres::PgPoolOptions::new()
        .max_connections(db_max_connections)
        .connect(explorer_db_url)
        .await
        .expect("failed to connect to the database");

    let balances_db_url = &std::env::var("BALANCES_DATABASE_URL")
        .expect("failed to get database url from BALANCES_DATABASE_URL env variable");
    let pool_balances = sqlx::postgres::PgPoolOptions::new()
        .max_connections(db_max_connections)
        .connect(balances_db_url)
        .await
        .expect("failed to connect to the balances database");

    let rpc_url =
        &std::env::var("RPC_URL").expect("failed to get RPC url from RPC_URL env variable");
    let rpc_client = near_jsonrpc_client::JsonRpcClient::connect(rpc_url);

    let config::Config {
        addr,
        cors_allowed_origins,
        limits,
    } = config::Config::default();

    let server = HttpServer::new(move || {
        let json_config = web::JsonConfig::default()
            .limit(limits.input_payload_max_size)
            .error_handler(|err, _req| {
                let error_message = err.to_string();
                actix_web::error::InternalError::from_response(
                    err,
                    errors::Error::from_error_kind(errors::ErrorKind::InvalidInput(error_message))
                        .error_response(),
                )
                .into()
            });

        let path_config = PathConfig::default().error_handler(|err, _| {
            let error_message = err.to_string();
            actix_web::error::InternalError::from_response(
                err,
                errors::Error::from_error_kind(errors::ErrorKind::InvalidInput(error_message))
                    .error_response(),
            )
            .into()
        });

        let mut spec = paperclip::v2::models::DefaultApiRaw::default();
        if let Ok(api_server_public_host) = std::env::var("API_SERVER_PUBLIC_HOST") {
            spec.schemes
                .insert(paperclip::v2::models::OperationProtocol::Https);
            spec.host = Some(api_server_public_host);
        }
        let base_path = get_api_base_path();
        spec.base_path = Some(base_path.clone());
        spec.info = paperclip::v2::models::Info {
            version: "0.1".into(),
            title: "NEAR Enhanced API powered by Pagoda".into(),
            description: Some(format!(r#"Try out our newly released Enhanced APIs - Balances (in Beta) and get what you need for all kinds of balances and token information at ease.
Call Enhanced APIs using the endpoint in the API URL box, varies by Network.

https://near-testnet.api.pagoda.co{base_path}

https://near-mainnet.api.pagoda.co{base_path}

Grab your API keys and give it a try! We will be adding more advanced Enhanced APIs in our offering, so stay tuned. Get the data you need without extra processing, NEAR Blockchain data query has never been easier!

We would love to hear from you on the data APIs you need, please leave feedback using the widget in the lower-right corner."#)),
            ..Default::default()
        };

        let mut app = App::new()
            .app_data(json_config)
            .app_data(path_config)
            .wrap(TracingLogger::default())
            .wrap(prometheus.clone())
            .app_data(web::Data::new(db_helpers::ExplorerPool(pool_explorer.clone())))
            .app_data(web::Data::new(db_helpers::BalancesPool(pool_balances.clone())))
            .app_data(web::Data::new(rpc_client.clone()))
            .wrap(get_cors(&cors_allowed_origins))
            .route("/", actix_web::web::get().to(playground_ui))
            .wrap_api_with_spec(spec);

        app = app.configure(modules::native::register_services);
        app = app.configure(modules::ft::register_services);
        app = app.configure(modules::nft::register_services);

        app.with_json_spec_at(format!("{base_path}/spec/v2.json").as_str())
            .with_json_spec_v3_at(format!("{base_path}/spec/v3.json").as_str())
            .build()
    })
    .bind(addr)
    .unwrap()
    .shutdown_timeout(5)
    .run();

    tracing::debug!(
        target: crate::LOGGER_MSG,
        "NEAR Enhanced API Server is starting..."
    );

    // opentelemetry::global::shutdown_tracer_provider();

    server.await
}

fn init_telemetry() {
    let env_filter = tracing_subscriber::EnvFilter::new(
        std::env::var("RUST_LOG")
            .as_deref()
            .unwrap_or("info,near=info,near_jsonrpc_client=warn,near_enhanced_api=debug"),
    );

    let opentelemetry = tracing_opentelemetry::layer();

    let stackdriver = tracing_stackdriver::layer().enable_cloud_trace(CloudTraceConfiguration {
        project_id: "my-project-id".to_owned(),
    });

    let subscriber = tracing_subscriber::Registry::default()
    .with(env_filter)
        .with(opentelemetry)
        .with(stackdriver);
    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to install `tracing` subscriber.")
}

// fn init_telemetry() {
//     let app_name = "pagoda-data-stack-dev";

//     // Start a new Jaeger trace pipeline.
//     // Spans are exported in batch - recommended setup for a production application.
//     global::set_text_map_propagator(TraceContextPropagator::new());
//     let tracer = opentelemetry_jaeger::new_pipeline()
//         .with_service_name(app_name)
//         .install_batch(TokioCurrentThread)
//         .expect("Failed to install OpenTelemetry tracer.");

//     // Filter based on level - trace, debug, info, warn, error
//     // Tunable via `RUST_LOG` env variable
//     let env_filter = tracing_subscriber::EnvFilter::new(
//         std::env::var("RUST_LOG")
//             .as_deref()
//             .unwrap_or("info,near=info,near_jsonrpc_client=warn,near_enhanced_api=debug"),
//     );
//     // Create a `tracing` layer using the Jaeger tracer
//     let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
//     // Create a `tracing` layer to emit spans as structured logs to stdout
//     let formatting_layer = BunyanFormattingLayer::new(app_name.into(), std::io::stdout);
//     // Combined them all together in a `tracing` subscriber
//     let subscriber = Registry::default()
//         .with(env_filter)
//         .with(telemetry)
//         .with(JsonStorageLayer)
//         .with(formatting_layer);
//     tracing::subscriber::set_global_default(subscriber)
//         .expect("Failed to install `tracing` subscriber.")
// }
