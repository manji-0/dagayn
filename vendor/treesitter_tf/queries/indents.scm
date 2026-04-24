; Indent inside block bodies, tuples, objects, and function calls
[
  (block_body)
  (tuple)
  (object)
  (function_call)
] @indent.begin

[
  "}"
  "]"
  ")"
] @indent.end

; Hanging indent for attribute values that start on the same line
(attribute
  "=" @indent.begin)
