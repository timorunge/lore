use quick_xml::XmlVersion;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use tracing::warn;

/// Parse RSS/Atom XML and extract all article URLs from `<item>` or `<entry>` elements.
pub(crate) fn parse_feed_urls(xml: &str) -> Vec<String> {
    // The reader is constructed from a &str, so the input is already decoded
    // UTF-8. Feeds using non-UTF-8 encodings declared in the XML declaration
    // will not be re-encoded -- callers must ensure UTF-8 input.
    let mut reader = Reader::from_str(xml);
    let mut urls = Vec::new();
    let mut in_item_or_entry = false;
    let mut in_link = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let local = e.local_name();
                let name = local.as_ref();
                match name {
                    "item" | "entry" => in_item_or_entry = true,
                    "link" if in_item_or_entry => in_link = true,
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let local = e.local_name();
                let name = local.as_ref();
                match name {
                    "item" | "entry" => in_item_or_entry = true,
                    "link" if in_item_or_entry => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == "href"
                                && let Ok(val) = attr.normalized_value(XmlVersion::Implicit1_0)
                            {
                                let url = val.trim().to_string();
                                if !url.is_empty() {
                                    urls.push(url);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) if in_link => {
                let url = e.xml_content(XmlVersion::Implicit1_0).trim().to_owned();
                if !url.is_empty() {
                    urls.push(url);
                }
                in_link = false;
            }
            Ok(Event::End(e)) => {
                let name = e.local_name();
                match name.as_ref() {
                    "item" | "entry" => {
                        in_item_or_entry = false;
                        in_link = false;
                    }
                    "link" => {
                        in_link = false;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!("feed XML parse error: {e}");
                break;
            }
            _ => {}
        }
    }

    urls.retain(|u| crate::util::is_http_url(u));
    urls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_parsing() {
        // RSS with <item><link> text nodes
        let xml = r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <item><link>https://example.com/post1</link></item>
    <item><link>https://example.com/post2</link></item>
  </channel>
</rss>"#;
        let urls = parse_feed_urls(xml);
        assert_eq!(
            urls,
            vec!["https://example.com/post1", "https://example.com/post2"]
        );

        // Atom with <entry><link href="..."/> attributes
        let xml = r#"<?xml version="1.0"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <link href="https://example.com/entry1"/>
  </entry>
  <entry>
    <link href="https://example.com/entry2"/>
  </entry>
</feed>"#;
        let urls = parse_feed_urls(xml);
        assert_eq!(
            urls,
            vec!["https://example.com/entry1", "https://example.com/entry2"]
        );

        // non-XML garbage -> empty (error recovery)
        let urls = parse_feed_urls("this is not xml");
        assert!(urls.is_empty());
    }
}
