use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::helpers::*;

#[tokio::test]
async fn sitemap_basic_urls() {
    let server = MockServer::start().await;
    mount_robots_404(&server).await;

    let port = server.address().port();
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://127.0.0.1:{port}/page1.txt</loc></url>
  <url><loc>http://127.0.0.1:{port}/page2.txt</loc></url>
</urlset>"#
    );

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sitemap_xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page1.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("Page one content about system configuration and setup.")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page2.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("Page two content about deployment and operations.")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let sitemap_url = format!("http://127.0.0.1:{port}/sitemap.xml");
    let config = format!(
        "name: sitemap-test\nsources:\n  - sitemap: {sitemap_url}\n    topic: Test\n\
         store:\n  path: test_index\n\
         processing:\n  max_chunk_chars: 800\n  min_chunk_chars: 20\n"
    );

    let config_path = run_ingest_with_loopback(dir.path(), &config);
    assert_store_counts(&config_path, 2, 2);
    assert_search_hit(&config_path, "configuration");
    assert_search_hit(&config_path, "deployment");
}

#[tokio::test]
async fn sitemap_include_filter() {
    let server = MockServer::start().await;
    mount_robots_404(&server).await;

    let port = server.address().port();
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://127.0.0.1:{port}/docs/guide.txt</loc></url>
  <url><loc>http://127.0.0.1:{port}/docs/reference.txt</loc></url>
  <url><loc>http://127.0.0.1:{port}/blog/post.txt</loc></url>
</urlset>"#
    );

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sitemap_xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/docs/guide.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("Guide content explaining how to use the system.")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/docs/reference.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("Reference content with API details and parameters.")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/blog/post.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("Blog post that should be excluded by the filter.")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let sitemap_url = format!("http://127.0.0.1:{port}/sitemap.xml");
    let config = format!(
        "name: sitemap-filter-test\nsources:\n  - sitemap: {sitemap_url}\n    \
         include: \"/docs/\"\n    topic: Test\n\
         store:\n  path: test_index\n\
         processing:\n  max_chunk_chars: 800\n  min_chunk_chars: 20\n"
    );

    let config_path = run_ingest_with_loopback(dir.path(), &config);
    assert_store_counts(&config_path, 2, 2);
}
