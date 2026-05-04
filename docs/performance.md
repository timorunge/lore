# Performance Tuning

This document explains how lore's ingest pipeline works internally and how to
tune it for large datasets. The guidance is based on stress-testing with the
Project Gutenberg archive (76,606 plain-text books, ~30 GB) -- a deliberately
extreme workload that pushes every part of the pipeline to its limits.

**The key thing to understand**: the initial ingest is the expensive operation.
Once your documents are indexed, everything else is fast:

- **Search** returns results in milliseconds (Tantivy BM25 over an optimized
  single-segment index).
- **Incremental updates** (`lore ingest`) skip unchanged documents via content
  hashing. Updating 10 documents in a 76K-document index takes seconds, not
  minutes.
- **MCP queries** serve concurrent requests with no serialization overhead
  (read-only access via `RwLock`).

The tuning guidance below helps you get through the initial ingest faster. For
most knowledge bases (hundreds to a few thousand documents), the defaults work
fine and ingest completes in seconds. If you are indexing tens of thousands of
documents or more, read on.

## Ingest pipeline overview

Ingest processes documents through a streaming pipeline with concurrent
stages connected by bounded channels:

```
Sources ──> [ Fetch ] ──> [ Process ] ──> [ Write ] ──> [ Optimize ]
             async          blocking       channel        merge
          buffer_unordered  thread pool    Tantivy        segments
          (concurrency)     (concurrency)  bg threads     into one
```

1. **Fetch** -- Load files from disk, URLs, S3, git, or other sources.
   Documents stream through as they complete via
   `buffer_unordered(concurrency)`.
2. **Process** -- Apply pipeline transforms (regex, metadata extraction),
   then chunk into pieces of up to `max_chunk_chars` characters. Chunking
   runs on a blocking thread pool with up to `concurrency` tasks in parallel.
3. **Write** -- Chunked documents are sent to the Tantivy index writer (a
   channel send into Tantivy's background indexing threads).
4. **Optimize** -- After all documents are ingested, merge Tantivy segments
   into fewer (ideally one) segments for fast search.

All stages run concurrently: documents flow through the pipeline individually
as they are ready, overlapping fetch, processing, and indexing work.

### Tantivy writer internals

`Index::writer(heap_size)` creates a writer with up to
`min(available_cores, 8)` background indexing threads (fewer if the
per-thread share would drop below 15 MB). The `heap_size` parameter is
the **total** budget, divided equally across threads. Each thread builds
an in-memory inverted index
and flushes it to a new segment when its share of the heap is full.

**Key insight**: A smaller per-thread heap means more frequent flushes, which
creates more intermediate segments. Each flush triggers the merge policy, which
spawns background merge operations that compete with indexing and chunking for
CPU. The `writer_heap_mb` setting (described below) controls this trade-off,
though the optimizer always produces a single segment in the end.

### The `en_stem` tokenizer

All searchable text fields (body, title, section, tags) use a tokenizer chain:
SimpleTokenizer -> RemoveLongFilter -> LowerCaser -> StopWordFilter -> Stemmer.
English stemming is the most expensive step. The cost scales with total text
volume across all chunks -- more chunks or larger chunks means more tokenization
work. This is the CPU-bound floor that cannot be reduced without changing the
tokenizer or dropping fields.

### Smart merge (optimize)

After ingest, lore optimizes the index by merging segments. The strategy adapts
to the segment distribution:

- **Dominant segment (>80% of docs)**: Only merge the smaller segments together.
  Avoids rewriting the large segment, keeping incremental updates cheap.
- **No dominant segment (e.g. after `--recreate`)**: Full merge of all segments.
  The merge reads and writes the full index once (I/O-bound).
- **Many segments (>256)**: Tree-shaped merge in groups of up to 256, then
  merge the results. Avoids Tantivy's per-merge fan-in limits.

Fewer segments before optimize = faster optimize.

## Configuration reference

All settings below are part of the standard `lore.yaml` config. Store-level
settings go under `store:`, processing settings under `processing:`. See
[Configuration](./configuration.md) for the full reference.

### `store.writer_heap_mb` (default: 256)

Total Tantivy writer heap in megabytes, divided across indexing threads.

On a machine with 8+ cores, `writer_heap_mb: 2048` gives each of the 8 indexing
threads 256 MB. A thread flushes to a new segment when its share is full --
larger shares mean fewer flushes, fewer intermediate segments, and less
background merge overhead during ingest.

| Value | Per-thread (8 cores) | Effect on large ingests |
|-------|---------------------|------------------------|
| 256 | 32 MB | Frequent flushes, more merge overhead. Fine for <10K docs. |
| 2048 | 256 MB | Infrequent flushes, minimal merge overhead. Best for large datasets. |
| 4096 | 512 MB | No measurable benefit over 2048 in benchmarks. |

Higher heap reduces intermediate merge overhead during ingest. The heap is lazy
-- memory is only allocated as documents are added, so setting 2048 on a small
dataset costs nothing.

### `store.phrase_search` (default: false)

Controls whether token positions are stored in the index.

- **`true`**: Stores token positions (`WithFreqsAndPositions`). Enables phrase
  queries like `"exact phrase"`. Larger index, slower indexing.
- **`false`**: Stores only term frequencies (`WithFreqs`). ~18% smaller index,
  ~18% faster indexing. BM25 ranking, multi-word queries, filters, and
  boosting all work identically. Only exact phrase matching is lost.

Only takes effect on `--recreate` or first-time index creation. Existing
indexes keep whatever setting they were created with.

### `store.doc_store_cache_blocks` (default: 500)

Size of Tantivy's document store LRU cache, in 16 KB blocks. The default
of 500 blocks = 8 MB. This cache holds decompressed blocks from the
on-disk doc store, speeding up random-access reads during search, `lore
read`, and `lore docs`.

| Value | Cache size | When to use |
|-------|-----------|-------------|
| 500 | 8 MB | Default. Sufficient for most knowledge bases. |
| 2000 | 32 MB | Large stores (>100K docs) with heavy concurrent search. |
| 100 | ~1.6 MB | Memory-constrained environments or small stores. |

Higher values improve read latency when accessing many different documents
in quick succession (e.g., MCP server under concurrent load). The cache is
per-store, so multi-KB setups multiply accordingly.

### `processing.concurrency` (default: half of available cores)

Number of parallel chunking tasks. Chunking is CPU-bound (text splitting,
regex transforms, BLAKE3 hashing). On a dedicated ingest machine, set this
to your core count. Chunking and Tantivy indexing overlap, so both benefit
from full core utilization.

### `processing.commit_interval` (default: 30)

Seconds between periodic Tantivy commits during ingest. Each commit:
1. Drains all indexing threads (flushes in-memory data to segments)
2. Reloads the reader (makes new segments searchable)
3. Runs garbage collection (deletes orphaned segment files)

Higher values reduce commit overhead but increase the data-loss window on
crash. For large bulk ingests, 60 is a good choice.

### `processing.max_chunk_chars` (default: 1600)

Maximum characters per chunk. Affects both search granularity and ingest speed:
- **Smaller (1000-1600)**: More chunks, finer search granularity, more Tantivy
  documents to index.
- **Larger (3000-6000)**: Fewer chunks, coarser results, less indexing work.
  The total text tokenized is roughly the same (same corpus), but per-chunk
  overhead (document creation, field encoding, channel sends) is reduced.

## Benchmark: Project Gutenberg (76,606 books)

Machine: Apple M1 Max, 10 cores, 64 GB RAM, SSD.
Dataset: 76,606 plain-text books (~30 GB).

### Profiles

Four configs are benchmarked. All share the same source files and pipeline.

| Profile | writer_heap_mb | phrase_search | concurrency | max_chunk_chars | commit_interval |
|---------|----------------|---------------|-------------|-----------------|-----------------|
| defaults | 256 | false | 5 | 1600 | 30 |
| tuned, phrase on | 2048 | true | 10 | 5000 | 60 |
| tuned, phrase off | 2048 | false | 10 | 5000 | 60 |
| tuned, large heap | 4096 | false | 10 | 5000 | 60 |

### Ingest results

| Profile | Chunks | Segments | Avg time | Min | Max | Index size | Peak mem |
|---------|--------|----------|----------|-----|-----|------------|----------|
| defaults | 22.9M | 1 | 24m 53s | 23m 22s | 26m 24s | 19.4 GB | 25.5 GB |
| tuned, phrase on |  7.5M | 1 | 10m 26s | 10m 10s | 10m 47s | 20.7 GB | 24.4 GB |
| tuned, phrase off |  7.5M | 1 |  8m 32s |  8m 30s |  8m 34s | 16.5 GB | 19.9 GB |
| tuned, large heap |  7.5M | 1 |  8m 17s |  8m 11s |  8m 20s | 16.5 GB | 21.8 GB |

All profiles produce a single optimized segment. Times are averages across
5 runs (2 for defaults). Min/max show run-to-run variance -- tuned profiles
are remarkably consistent (within 4 seconds).

Effect of tuning (defaults -> tuned, phrase off):
- 66% faster wall time (24m 53s -> 8m 32s)
- 15% smaller index (19.4 GB -> 16.5 GB)
- 22% lower peak memory (25.5 GB -> 19.9 GB)

The gap is primarily from `max_chunk_chars` (1600 vs 5000 creates 3x more
chunks) and `concurrency` (5 vs 10 on a 10-core machine). The `writer_heap_mb`
difference has less impact -- as shown by tuned-large-heap vs tuned-phrase-off, doubling
the heap from 2048 to 4096 yields no measurable improvement.

Effect of phrase search (tuned on vs off):
- 18% faster wall time without phrase (10m 26s -> 8m 32s)
- 20% smaller index (20.7 GB -> 16.5 GB)
- 18% lower peak memory (24.4 GB -> 19.9 GB)

### Where the time goes

At ~8.5 minutes with tuned settings (phrase off, 10 cores):
- **Tantivy tokenization** (~5-6 min): 7.5M chunks through the `en_stem`
  tokenizer across 8 indexing threads. This is the CPU-bound floor.
- **Chunking** (~2-3 min): Text splitting across 10 concurrent blocking tasks.
  Overlaps with Tantivy indexing via streaming pipeline.
- **Optimize** (~1 min): Merging intermediate segments into 1 (~17 GB
  read + write, I/O-bound).
- **File I/O** (<1 min): Reading 76K files from SSD, overlapped with processing.

With defaults (5 concurrency, 1600-char chunks), the 3x chunk count triples
tokenization work and the lower concurrency reduces parallelism.

### Incremental updates

A no-op incremental ingest (all 76K documents unchanged) completes in ~3.1
seconds across all profiles. The cost is dominated by loading the stamps cache
and stat-checking files -- no content is read or indexed.

### Search latency

Plain-text queries against 7.5M chunks (tuned profiles) and 22.9M chunks
(defaults), averaged over 5 repetitions:

| Query | defaults (22.9M) | tuned, phrase on (7.5M) | tuned, phrase off (7.5M) |
|-------|-----------------:|------------------------:|-------------------------:|
| married | 96 ms | 79 ms | 89 ms |
| the nature of things | 120 ms | 96 ms | 106 ms |
| quantum mechanics | 91 ms | 78 ms | 86 ms |
| declaration independence | 95 ms | 81 ms | 87 ms |
| error | 92 ms | 77 ms | 86 ms |
| love AND death | 123 ms | 101 ms | 110 ms |
| französische revolution | 96 ms | 78 ms | 85 ms |

Filtered queries (tuned, phrase off) add overhead proportional to filter
complexity:

| Filter | Example query | Avg ms |
|--------|---------------|-------:|
| _(none)_ | wisdom | 97 ms |
| `--lang en` | wisdom | 97 ms |
| `--source gutenberg` | wisdom | 250 ms |
| `--topic Middlemarch` | wisdom | 127 ms |
| `--max-per-source 1` | wisdom | 306 ms |
| `--max-per-source 2` | wisdom | 446 ms |

`--max-per-source` is the most expensive filter because it requires scoring and
deduplication across all matching documents.

### Tuning for speed vs. search quality

For maximum ingest speed (benchmarking, bulk loads):
```yaml
store:
  writer_heap_mb: 2048
  phrase_search: false

processing:
  max_chunk_chars: 5000
  concurrency: 10       # match your core count
  commit_interval: 60
```

For best search quality (production):
```yaml
store:
  writer_heap_mb: 2048
  phrase_search: true

processing:
  max_chunk_chars: 1600
  commit_interval: 30
```

## Troubleshooting

### Store is larger than expected

After ingest, check the file count in the tantivy directory. If there are
hundreds of files but `meta.json` references only 1-2 segments, orphaned
segment files were not garbage-collected. This can happen if:
- The process was killed during optimize
- A bug prevented GC from running after commit

Fix: run `lore maintain compact` to merge segments and trigger garbage
collection. If that doesn't resolve it, `lore ingest --recreate` rebuilds
the store from scratch.

### Optimize takes too long

Optimize time is proportional to store size (it rewrites the full index once).
To reduce optimize time:
1. Use `max_chunk_chars: 3000-5000` to reduce total chunk count and index size
2. Set `store.phrase_search: false` to reduce index size ~18%

### Ingest is CPU-bound but cores are idle

If `cpu_time / wall_time` is much less than your core count, increase
`processing.concurrency` to match your core count. Also check
`store.writer_heap_mb` -- values below 1024 can cause excessive intermediate
flushes whose background merges steal CPU from chunking.

### Update mode is slow on unchanged data

lore uses a three-tier freshness check to skip unchanged documents:
1. **mtime + file size** (local files only): O(1), no content read needed
2. **ETag / Last-Modified** (URL/S3 sources): cached from previous ingest
3. **BLAKE3 content hash**: authoritative fallback, requires reading the file

If all documents are unchanged, update mode should complete in seconds (only
the freshness checks run, no chunking or indexing). If it's slow, files may
have been touched without changing content (e.g. `cp -p`). Run
`lore maintain check` to verify store consistency. Use `--force` to
re-process all documents, or `--recreate` to rebuild from scratch.
