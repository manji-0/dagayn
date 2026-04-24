terraform {
  required_version = ">= 1.5.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

provider "aws" {
  region = "us-east-1"
}

variable "tags" {
  type = map(string)
  default = {
    Environment = "prod"
  }
}

locals {
  common_tags = merge(var.tags, {
    ManagedBy = "dagayn"
  })
}

module "network" {
  source = "./modules/network"
}

data "aws_caller_identity" "current" {}

resource "aws_vpc" "main" {
  cidr_block = module.network.cidr_block

  tags = merge(local.common_tags, {
    Account = data.aws_caller_identity.current.account_id
  })
}

check "vpc_ready" {
  assert {
    condition = length(module.network.public_subnet_ids) > 0
    error_message = "network module must expose subnets"
  }
}

output "vpc_id" {
  value = aws_vpc.main.id
}
