#include "../../common/scanner.h"

void *tree_sitter_botwork_suite_external_scanner_create(void) { return NULL; }
void tree_sitter_botwork_suite_external_scanner_destroy(void *payload) { (void)payload; }
unsigned tree_sitter_botwork_suite_external_scanner_serialize(void *payload, char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}
void tree_sitter_botwork_suite_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}
bool tree_sitter_botwork_suite_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  return scan_botwork(payload, lexer, valid_symbols);
}
