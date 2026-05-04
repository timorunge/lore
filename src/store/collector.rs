use std::collections::HashMap;

use tantivy::collector::{Collector, SegmentCollector};
use tantivy::columnar::StrColumn;
use tantivy::{DocAddress, DocId, Score, SegmentOrdinal, SegmentReader};

use crate::store::schema::fields;

/// Tantivy collector that enforces a per-source cap during scoring.
pub(crate) struct DiversityCollector {
    max_per_source: usize,
    limit: usize,
}

/// Per-segment collector that tracks source ordinal counts.
pub(crate) struct DiversitySegmentCollector {
    max_per_source: usize,
    segment_ord: u32,
    source_col: Option<StrColumn>,
    ordinal_counts: HashMap<u64, usize>,
    hits: Vec<(f32, DocAddress)>,
    accepted_ords: Vec<u64>,
}

impl DiversityCollector {
    /// Create a collector that caps results per source document.
    pub(crate) fn new(max_per_source: usize, limit: usize) -> Self {
        Self {
            max_per_source,
            limit,
        }
    }
}

impl Collector for DiversityCollector {
    type Fruit = Vec<(f32, DocAddress)>;
    type Child = DiversitySegmentCollector;

    fn for_segment(
        &self,
        segment_local_id: SegmentOrdinal,
        segment: &SegmentReader,
    ) -> tantivy::Result<Self::Child> {
        let source_col = segment.fast_fields().str(fields::SOURCE)?;
        Ok(DiversitySegmentCollector {
            max_per_source: self.max_per_source,
            segment_ord: segment_local_id,
            source_col,
            ordinal_counts: HashMap::new(),
            hits: Vec::new(),
            accepted_ords: Vec::new(),
        })
    }

    fn requires_scoring(&self) -> bool {
        true
    }

    fn merge_fruits(
        &self,
        segment_fruits: Vec<Vec<(f32, DocAddress, String)>>,
    ) -> tantivy::Result<Vec<(f32, DocAddress)>> {
        let total: usize = segment_fruits.iter().map(Vec::len).sum();
        let mut all: Vec<(f32, DocAddress, String)> = Vec::with_capacity(total);
        all.extend(segment_fruits.into_iter().flatten());

        all.sort_unstable_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.segment_ord.cmp(&b.1.segment_ord))
                .then_with(|| a.1.doc_id.cmp(&b.1.doc_id))
        });

        let mut source_counts: HashMap<&str, usize> = HashMap::new();
        let mut hits = Vec::with_capacity(self.limit);

        for (score, addr, source) in &all {
            let count = source_counts.entry(source.as_str()).or_default();
            *count += 1;
            if *count <= self.max_per_source {
                hits.push((*score, *addr));
                if hits.len() >= self.limit {
                    break;
                }
            }
        }

        Ok(hits)
    }
}

impl SegmentCollector for DiversitySegmentCollector {
    type Fruit = Vec<(f32, DocAddress, String)>;

    fn collect(&mut self, doc: DocId, score: Score) {
        let ord = self
            .source_col
            .as_ref()
            .and_then(|col| col.term_ords(doc).next())
            .unwrap_or(u64::MAX);
        let count = self.ordinal_counts.entry(ord).or_default();
        *count += 1;
        if *count > self.max_per_source {
            return;
        }
        self.hits
            .push((score, DocAddress::new(self.segment_ord, doc)));
        self.accepted_ords.push(ord);
    }

    fn harvest(self) -> Self::Fruit {
        self.hits
            .into_iter()
            .zip(self.accepted_ords)
            .map(|((score, addr), ord)| {
                let source = self
                    .source_col
                    .as_ref()
                    .and_then(|col| {
                        let mut s = String::new();
                        col.ord_to_str(ord, &mut s).ok()?;
                        Some(s)
                    })
                    .unwrap_or_default();
                (score, addr, source)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use tantivy::collector::Collector;

    use super::*;

    #[test]
    fn merge_fruits_applies_per_source_cap() {
        let collector = DiversityCollector::new(1, 3);
        let segment: Vec<(f32, DocAddress, String)> = vec![
            (10.0, DocAddress::new(0, 0), "A".into()),
            (8.0, DocAddress::new(0, 1), "B".into()),
            (6.0, DocAddress::new(0, 2), "C".into()),
            (4.0, DocAddress::new(0, 3), "A".into()),
            (2.0, DocAddress::new(0, 4), "B".into()),
            (1.0, DocAddress::new(0, 5), "C".into()),
        ];
        let result = collector.merge_fruits(vec![segment]).unwrap();
        assert_eq!(result.len(), 3);
        let scores: Vec<f32> = result.iter().map(|(s, _)| *s).collect();
        assert_eq!(scores, vec![10.0, 8.0, 6.0]);
    }
}
