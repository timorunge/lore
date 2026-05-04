use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::helpers::*;

#[tokio::test]
async fn rss_feed_ingested() {
    let server = MockServer::start().await;
    mount_robots_404(&server).await;

    let port = server.address().port();
    let rss_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Feed</title>
    <item>
      <title>Article One</title>
      <link>http://127.0.0.1:{port}/article1.txt</link>
    </item>
    <item>
      <title>Article Two</title>
      <link>http://127.0.0.1:{port}/article2.txt</link>
    </item>
  </channel>
</rss>"#
    );

    Mock::given(method("GET"))
        .and(path("/feed.rss"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(rss_xml)
                .insert_header("content-type", "application/rss+xml"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/article1.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("First article discussing software architecture patterns.")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/article2.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("Second article covering testing strategies and approaches.")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let feed_url = format!("http://127.0.0.1:{port}/feed.rss");
    let config = format!(
        "name: rss-test\nsources:\n  - feed: {feed_url}\n    topic: Test\n\
         store:\n  path: test_index\n\
         processing:\n  max_chunk_chars: 800\n  min_chunk_chars: 20\n"
    );

    let config_path = run_ingest_with_loopback(dir.path(), &config);
    assert_store_counts(&config_path, 2, 2);
    assert_search_hit(&config_path, "architecture");
    assert_search_hit(&config_path, "testing strategies");
}

#[tokio::test]
async fn atom_feed_ingested() {
    let server = MockServer::start().await;
    mount_robots_404(&server).await;

    let port = server.address().port();
    let atom_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Test Atom Feed</title>
  <entry>
    <title>Entry One</title>
    <link href="http://127.0.0.1:{port}/entry1.txt"/>
  </entry>
  <entry>
    <title>Entry Two</title>
    <link href="http://127.0.0.1:{port}/entry2.txt"/>
  </entry>
</feed>"#
    );

    Mock::given(method("GET"))
        .and(path("/feed.atom"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(atom_xml)
                .insert_header("content-type", "application/atom+xml"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/entry1.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("First entry about distributed systems and scalability.")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/entry2.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("Second entry about observability and monitoring tools.")
                .insert_header("content-type", "text/plain"),
        )
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let feed_url = format!("http://127.0.0.1:{port}/feed.atom");
    let config = format!(
        "name: atom-test\nsources:\n  - feed: {feed_url}\n    topic: Test\n\
         store:\n  path: test_index\n\
         processing:\n  max_chunk_chars: 800\n  min_chunk_chars: 20\n"
    );

    let config_path = run_ingest_with_loopback(dir.path(), &config);
    assert_store_counts(&config_path, 2, 2);
    assert_search_hit(&config_path, "distributed systems");
    assert_search_hit(&config_path, "observability");
}
