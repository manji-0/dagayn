; resource "type" "name" { ... }
(resource_block
  type: (string_lit) @name) @definition.class

; data "type" "name" { ... }
(data_block
  type: (string_lit) @name) @definition.class

; variable "name" { ... }
(variable_block
  name: (string_lit) @name) @definition.var

; output "name" { ... }
(output_block
  name: (string_lit) @name) @definition.var

; module "name" { ... }
(module_block
  name: (string_lit) @name) @definition.module

; locals { ... }
(locals_block) @definition.namespace

; provider "name" { ... }
(provider_block
  name: (string_lit) @name) @definition.interface

; ephemeral "type" "name" { ... }
(ephemeral_block
  type: (string_lit) @name) @definition.class
