#!/usr/bin/env python3
"""Minimal exec source example: emit JSONL documents to stdout."""
import json

DOCS = [
    {
        "source": "exec-demo/kraken",
        "title": "The Kraken",
        "content": (
            "Below the thunders of the upper deep,\n"
            "Far, far beneath in the abysmal sea,\n"
            "His ancient, dreamless, uninvaded sleep\n"
            "The Kraken sleepeth.\n\n"
            "-- Alfred, Lord Tennyson (1830)"
        ),
        "tags": ["poetry", "kraken"],
        "lang": "en",
    },
    {
        "source": "exec-demo/origins",
        "title": "Kraken Origins",
        "content": (
            "The Kraken is a legendary sea monster from Scandinavian folklore. "
            "First described in a 1180 Norwegian text, the creature was said to "
            "dwell off the coasts of Norway and Greenland. Sailors reported it as "
            "an island-sized beast capable of dragging entire ships beneath the waves."
        ),
        "tags": ["folklore", "kraken"],
        "lang": "en",
    },
    {
        "source": "exec-demo/change-detection",
        "title": "Change Detection",
        "content": (
            "Q: How does change detection work with exec sources?\n"
            "A: The command runs on every ingest, but lore hashes each document's "
            "content and skips re-indexing anything that hasn't changed."
        ),
        "lang": "en",
    },
]

for doc in DOCS:
    print(json.dumps(doc))
