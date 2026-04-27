# Mixed Fixture

This repo exercises cross-artifact edge extraction.

## Graph API

Call `build_graph` to construct a graph from a repository path.
Use `run_analysis` to print a summary.

## Infrastructure

The `aws_s3_bucket.graph_store` resource holds graph snapshots.
