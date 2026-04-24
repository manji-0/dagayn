# advanced.tf — Demonstrates complex Terraform patterns parsed by treesitter-tf

# Terraform settings with required_providers
terraform {
  required_version = ">= 1.5"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }

  backend "s3" {
    bucket = "my-tfstate"
    key    = "prod/terraform.tfstate"
    region = "us-east-1"
  }
}

# Variable with complex type constraint
variable "network_config" {
  description = "Network configuration"
  type = object({
    vpc_cidr     = string
    subnet_cidrs = list(string)
    enable_ipv6  = optional(bool, false)
  })
}

variable "tags" {
  type    = map(string)
  default = {}
}

# Locals with complex expressions
locals {
  # Nested function calls
  unique_azs = tolist(toset(data.aws_availability_zones.available.names))

  # For-object expression with condition
  public_subnets = {
    for idx, cidr in var.network_config.subnet_cidrs :
    "subnet-${idx}" => cidr
    if idx < 3
  }

  # Multi-line string interpolation
  bucket_name = "${var.tags["Environment"]}-${var.tags["Project"]}-artifacts"

  # try() for safe attribute access
  ipv6_enabled = try(var.network_config.enable_ipv6, false)

  # Complex merge
  default_tags = merge(var.tags, {
    ManagedBy  = "terraform"
    LastUpdate = "2024-01-01"
  })
}

# Data sources
data "aws_availability_zones" "available" {
  state = "available"
}

data "aws_caller_identity" "current" {}

data "aws_iam_policy_document" "bucket_policy" {
  statement {
    sid    = "AllowAccountAccess"
    effect = "Allow"

    principals {
      type        = "AWS"
      identifiers = ["arn:aws:iam::${data.aws_caller_identity.current.account_id}:root"]
    }

    actions = [
      "s3:GetObject",
      "s3:PutObject",
      "s3:DeleteObject",
    ]

    resources = ["arn:aws:s3:::${local.bucket_name}/*"]
  }
}

# Resource with dynamic block and lifecycle
resource "aws_s3_bucket" "artifacts" {
  bucket = local.bucket_name

  tags = local.default_tags

  lifecycle {
    prevent_destroy = true
    ignore_changes  = [tags["LastUpdate"]]
  }
}

resource "aws_s3_bucket_policy" "artifacts" {
  bucket = aws_s3_bucket.artifacts.id
  policy = data.aws_iam_policy_document.bucket_policy.json
}

# Resource with count and splat
resource "aws_subnet" "public" {
  count = length(var.network_config.subnet_cidrs)

  vpc_id            = aws_vpc.main.id
  cidr_block        = var.network_config.subnet_cidrs[count.index]
  availability_zone = local.unique_azs[count.index % length(local.unique_azs)]

  tags = merge(local.default_tags, {
    Name = "public-${count.index}"
    Type = "public"
  })
}

# Resource with for_each
resource "aws_iam_user" "service_accounts" {
  for_each = toset(["deploy", "ci", "monitoring"])

  name = "${each.key}-sa"
  path = "/service-accounts/"

  tags = merge(local.default_tags, {
    Role = each.key
  })
}

# Dynamic block with iterator
resource "aws_security_group" "app" {
  name   = "${local.bucket_name}-sg"
  vpc_id = aws_vpc.main.id

  dynamic "ingress" {
    for_each = {
      http  = 80
      https = 443
    }
    iterator = rule

    content {
      from_port   = rule.value
      to_port     = rule.value
      protocol    = "tcp"
      cidr_blocks = [var.network_config.vpc_cidr]
      description = "Allow ${rule.key}"
    }
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = local.default_tags
}

# Module call
module "vpc" {
  source = "../modules/vpc"

  cidr_block = var.network_config.vpc_cidr
  tags       = local.default_tags

  depends_on = [aws_s3_bucket.artifacts]
}

# Check block (Terraform 1.5+)
check "bucket_accessible" {
  assert {
    condition     = aws_s3_bucket.artifacts.bucket_domain_name != ""
    error_message = "Artifacts bucket domain name should not be empty."
  }
}

# Moved block
moved {
  from = aws_s3_bucket.old_artifacts
  to   = aws_s3_bucket.artifacts
}

# Outputs with complex expressions
output "subnet_ids" {
  description = "Public subnet IDs"
  value       = aws_subnet.public[*].id
}

output "service_account_arns" {
  description = "IAM user ARNs for service accounts"
  value       = { for k, v in aws_iam_user.service_accounts : k => v.arn }
}

output "bucket_arn" {
  description = "Artifacts bucket ARN"
  value       = aws_s3_bucket.artifacts.arn
  sensitive   = false
}
