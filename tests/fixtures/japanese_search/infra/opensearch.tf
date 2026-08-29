# 検索クラスタ。ドキュメントの自然言語検索とは別ノード。
resource "aws_opensearch_domain" "logs" {
  domain_name    = "ops-search"
  engine_version = "OpenSearch_2.11"
}

variable "retention_days" {
  type    = number
  default = 14
}
