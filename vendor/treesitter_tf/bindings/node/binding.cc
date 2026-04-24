#include <napi.h>

typedef struct TSLanguage TSLanguage;

extern "C" TSLanguage *tree_sitter_terraform();

// "tree-sitter", "language" hides the implementation of the Language class from
// the node module, so it is not accessible from JavaScript.
Napi::Object Init(Napi::Env env, Napi::Object exports) {
  exports["name"] = Napi::String::New(env, "terraform");
  auto language = Napi::External<TSLanguage>::New(env, tree_sitter_terraform());
  exports["language"] = language;
  return exports;
}

NODE_API_MODULE(tree_sitter_terraform_binding, Init)
