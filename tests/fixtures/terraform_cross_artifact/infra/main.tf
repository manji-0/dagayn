resource "null_resource" "bootstrap" {
  provisioner "local-exec" {
    command = "python3 ${path.module}/../scripts/bootstrap.py"
  }
}

resource "aws_lambda_function" "auth" {
  filename = "${path.module}/../app/hello.py"
  handler  = "hello.main"
  runtime  = "python3.12"
  role     = "arn:aws:iam::123456789012:role/lambda"
}

data "archive_file" "hello_zip" {
  type        = "zip"
  source_dir  = "${path.module}/../app"
  output_path = "${path.module}/hello.zip"
}

resource "google_cloudfunctions_function" "api" {
  name                = "api"
  runtime             = "python312"
  entry_point         = "serve"
  source_directory    = "${path.module}/../app"
  available_memory_mb = 128
  trigger_http        = true
}
