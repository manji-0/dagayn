from __future__ import annotations

import argparse

from dagayn.cli.commands.build import _print_local_embedding_summary, register_commands


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command")
    register_commands(sub)
    return parser


def test_build_parser_accepts_local_embedding_options():
    args = _parser().parse_args(
        [
            "build",
            "--local-embedding",
            "high",
            "--local-embedding-port",
            "19090",
            "--local-embedding-bin",
            "/tmp/llama-server",
            "--keep-local-embedding-server",
            "--local-embedding-timeout",
            "12",
            "--local-embedding-request-timeout",
            "17",
            "--local-embedding-batch-size",
            "8",
        ]
    )

    assert args.command == "build"
    assert args.local_embedding == "high"
    assert args.local_embedding_port == 19090
    assert args.local_embedding_bin == "/tmp/llama-server"
    assert args.keep_local_embedding_server is True
    assert args.local_embedding_timeout == 12
    assert args.local_embedding_request_timeout == 17
    assert args.local_embedding_batch_size == 8


def test_update_parser_accepts_local_embedding_options():
    args = _parser().parse_args(["update", "--local-embedding", "low"])

    assert args.command == "update"
    assert args.local_embedding == "low"
    assert args.local_embedding_port == 18080
    assert args.local_embedding_bin == "llama-server"
    assert args.keep_local_embedding_server is False
    assert args.local_embedding_timeout == 300
    assert args.local_embedding_request_timeout == 60
    assert args.local_embedding_batch_size == 1


def test_print_local_embedding_summary_includes_orphan_count(capsys):
    _print_local_embedding_summary(
        {
            "local_embedding": {
                "preset": "high",
                "server_started": False,
                "newly_embedded": 1,
                "orphans_removed": 2,
                "total_embeddings": 9,
            }
        }
    )

    assert "1 new, 2 orphan removed, 9 total" in capsys.readouterr().out
