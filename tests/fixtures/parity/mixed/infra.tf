variable "environment" {
  type    = string
  default = "dev"
}

resource "aws_s3_bucket" "graph_store" {
  bucket = "dagayn-graph-${var.environment}"
}
