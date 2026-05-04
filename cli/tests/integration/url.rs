use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::helpers::*;

#[tokio::test]
async fn url_plain_text_ingested() {
    let server = MockServer::start().await;
    mount_robots_404(&server).await;

    Mock::given(method("GET"))
        .and(path("/doc.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(
                    "Lore is a local knowledge base tool for ingesting and searching documents.",
                )
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let url = format!("http://127.0.0.1:{}/doc.txt", server.address().port());
    let config = format!(
        "name: url-test\nsources:\n  - url: {url}\n    topic: Test\nstore:\n  path: test_index\n\
         processing:\n  max_chunk_chars: 800\n  min_chunk_chars: 20\n"
    );

    let config_path = run_ingest_with_loopback(dir.path(), &config);
    assert_store_counts(&config_path, 1, 1);
    assert_search_hit(&config_path, "knowledge base");
}

#[tokio::test]
async fn url_html_content_extracted() {
    let server = MockServer::start().await;
    mount_robots_404(&server).await;

    Mock::given(method("GET"))
        .and(path("/page.html"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(
                    "<html><head><title>Test Page</title></head>\
                     <body><h1>Architecture Guide</h1>\
                     <p>This document explains the modular system architecture.</p>\
                     </body></html>",
                )
                .insert_header("content-type", "text/html"),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let url = format!("http://127.0.0.1:{}/page.html", server.address().port());
    let config = format!(
        "name: url-html-test\nsources:\n  - url: {url}\n    topic: Test\nstore:\n  path: test_index\n\
         processing:\n  max_chunk_chars: 800\n  min_chunk_chars: 20\n"
    );

    let config_path = run_ingest_with_loopback(dir.path(), &config);
    assert_store_counts(&config_path, 1, 1);
    assert_search_hit(&config_path, "architecture");
}

#[tokio::test]
async fn url_multiple_urls_ingested() {
    let server = MockServer::start().await;
    mount_robots_404(&server).await;

    for (p, body) in [
        ("/alpha.txt", "Alpha document content about configuration."),
        ("/beta.txt", "Beta document content about deployment."),
        ("/gamma.txt", "Gamma document content about monitoring."),
    ] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(body)
                    .insert_header("content-type", "text/plain"),
            )
            .mount(&server)
            .await;
    }

    let dir = TempDir::new().unwrap();
    let port = server.address().port();
    let config = [
        "name: url-multi-test",
        "sources:",
        "  - url:",
        &format!("      - http://127.0.0.1:{port}/alpha.txt"),
        &format!("      - http://127.0.0.1:{port}/beta.txt"),
        &format!("      - http://127.0.0.1:{port}/gamma.txt"),
        "    topic: Test",
        "store:",
        "  path: test_index",
        "processing:",
        "  max_chunk_chars: 800",
        "  min_chunk_chars: 20",
        "",
    ]
    .join("\n");

    let config_path = run_ingest_with_loopback(dir.path(), &config);
    assert_store_counts(&config_path, 3, 3);
    assert_search_hit(&config_path, "configuration");
    assert_search_hit(&config_path, "deployment");
    assert_search_hit(&config_path, "monitoring");
}

#[tokio::test]
async fn url_404_does_not_crash() {
    let server = MockServer::start().await;
    mount_robots_404(&server).await;

    Mock::given(method("GET"))
        .and(path("/good.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("Good document with searchable content about indexing.")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/missing.txt"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let port = server.address().port();
    let config = [
        "name: url-404-test",
        "sources:",
        "  - url:",
        &format!("      - http://127.0.0.1:{port}/good.txt"),
        &format!("      - http://127.0.0.1:{port}/missing.txt"),
        "    topic: Test",
        "store:",
        "  path: test_index",
        "processing:",
        "  max_chunk_chars: 800",
        "  min_chunk_chars: 20",
        "",
    ]
    .join("\n");

    let config_path = run_ingest_with_loopback(dir.path(), &config);
    // The good URL should be indexed; the 404 should be skipped without crashing.
    assert_store_counts(&config_path, 1, 1);
    assert_search_hit(&config_path, "indexing");
}
