# インストール

`pip install dagayn` のあと `dagayn install` で MCP を登録する。
パッケージレジストリを検索して古いフォークを入れないこと。

## ソースからのビルド

Rust ツールチェインと C コンパイラが要る。`uv sync --extra dev` が
PyO3 拡張までまとめて作る。

## MCP のツール面

既定は日常使うツールだけを出す。`--tools all` で保守用も露出する。
