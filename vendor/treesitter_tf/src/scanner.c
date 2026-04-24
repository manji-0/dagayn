/**
 * External scanner for tree-sitter-terraform
 *
 * Handles:
 *   - Quoted string delimiters and template literal chunks
 *   - Template interpolation ${...}
 *   - Template directive %{...}
 *   - Heredoc identifiers (opening and closing markers)
 *
 * Critical contract with tree-sitter:
 *   lexer->advance(lexer, skip)  — consume current char (tentatively)
 *   lexer->mark_end(lexer)       — commit all consumed chars as the token end
 *
 * If we advance past character X but never call mark_end after that, X is NOT
 * included in the token (lexer rewinds to the last mark_end position).
 * We use this to "peek" at the character after $ or % to decide whether we're
 * looking at an interpolation/directive start without including the $ or % in
 * the literal chunk when they turn out to be interpolation starts.
 */

#include <assert.h>
#include <stdio.h>
#include <string.h>
#include "tree_sitter/parser.h"
#include "tree_sitter/alloc.h"
#include "tree_sitter/array.h"

// ============================================================================
// Token types (must match the order of externals in grammar.js)
// ============================================================================

typedef enum {
    TOKEN_QUOTED_TEMPLATE_START,        // "
    TOKEN_QUOTED_TEMPLATE_END,          // "
    TOKEN_TEMPLATE_LITERAL_CHUNK,       // raw text inside template
    TOKEN_TEMPLATE_INTERPOLATION_START, // ${
    TOKEN_TEMPLATE_INTERPOLATION_END,   // }
    TOKEN_TEMPLATE_DIRECTIVE_START,     // %{
    TOKEN_TEMPLATE_DIRECTIVE_END,       // }
    TOKEN_HEREDOC_IDENTIFIER,           // the identifier after << or <<-
} TokenType;

// ============================================================================
// Context types
// ============================================================================

typedef enum {
    CTX_QUOTED,         // inside "..."
    CTX_HEREDOC,        // inside <<IDENT...IDENT
    CTX_INTERP,         // inside ${...}
    CTX_DIRECTIVE,      // inside %{...}
} ContextType;

typedef Array(char) String;

typedef struct {
    ContextType type;
    String heredoc_id; // only used when type == CTX_HEREDOC
} Context;

typedef Array(Context) ContextStack;

typedef struct {
    ContextStack stack;
} Scanner;

// ============================================================================
// Utility helpers
// ============================================================================

static void context_free(Context *ctx) {
    if (ctx->type == CTX_HEREDOC) {
        array_delete(&ctx->heredoc_id);
    }
}

static bool in_template_context(const Scanner *scanner) {
    if (scanner->stack.size == 0) return false;
    ContextType top = scanner->stack.contents[scanner->stack.size - 1].type;
    return top == CTX_QUOTED || top == CTX_HEREDOC;
}

// ============================================================================
// Interface functions
// ============================================================================

void *tree_sitter_terraform_external_scanner_create(void) {
    Scanner *scanner = ts_malloc(sizeof(Scanner));
    array_init(&scanner->stack);
    return scanner;
}

void tree_sitter_terraform_external_scanner_destroy(void *payload) {
    Scanner *scanner = (Scanner *)payload;
    for (uint32_t i = 0; i < scanner->stack.size; i++) {
        context_free(&scanner->stack.contents[i]);
    }
    array_delete(&scanner->stack);
    ts_free(scanner);
}

unsigned tree_sitter_terraform_external_scanner_serialize(void *payload, char *buffer) {
    Scanner *scanner = (Scanner *)payload;
    unsigned offset = 0;

    if (offset + 1 > TREE_SITTER_SERIALIZATION_BUFFER_SIZE) return 0;
    buffer[offset++] = (char)scanner->stack.size;

    for (uint32_t i = 0; i < scanner->stack.size; i++) {
        Context *ctx = &scanner->stack.contents[i];
        if (offset + 1 > TREE_SITTER_SERIALIZATION_BUFFER_SIZE) return 0;
        buffer[offset++] = (char)ctx->type;

        if (ctx->type == CTX_HEREDOC) {
            uint32_t len = ctx->heredoc_id.size;
            if (offset + 1 + len > TREE_SITTER_SERIALIZATION_BUFFER_SIZE) return 0;
            buffer[offset++] = (char)len;
            memcpy(buffer + offset, ctx->heredoc_id.contents, len);
            offset += len;
        }
    }
    return offset;
}

void tree_sitter_terraform_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
    Scanner *scanner = (Scanner *)payload;

    for (uint32_t i = 0; i < scanner->stack.size; i++) {
        context_free(&scanner->stack.contents[i]);
    }
    array_clear(&scanner->stack);

    if (length == 0) return;

    unsigned offset = 0;
    if (offset >= length) return;
    uint8_t stack_size = (uint8_t)buffer[offset++];

    for (uint8_t i = 0; i < stack_size; i++) {
        if (offset >= length) break;
        ContextType type = (ContextType)(uint8_t)buffer[offset++];
        Context ctx;
        ctx.type = type;
        array_init(&ctx.heredoc_id);

        if (type == CTX_HEREDOC) {
            if (offset >= length) break;
            uint8_t len = (uint8_t)buffer[offset++];
            for (uint8_t j = 0; j < len && offset < length; j++) {
                array_push(&ctx.heredoc_id, buffer[offset++]);
            }
        }
        array_push(&scanner->stack, ctx);
    }
}

// ============================================================================
// Main scan function
// ============================================================================

bool tree_sitter_terraform_external_scanner_scan(
    void *payload,
    TSLexer *lexer,
    const bool *valid_symbols
) {
    Scanner *scanner = (Scanner *)payload;

    // ------------------------------------------------------------------
    // template_interpolation_end / template_directive_end
    // Must check BEFORE skipping whitespace since } is the token.
    // ------------------------------------------------------------------
    if (scanner->stack.size > 0) {
        Context *top = &scanner->stack.contents[scanner->stack.size - 1];

        if (top->type == CTX_INTERP &&
            valid_symbols[TOKEN_TEMPLATE_INTERPOLATION_END] &&
            lexer->lookahead == '}') {
            lexer->advance(lexer, false);
            lexer->mark_end(lexer);
            context_free(top);
            scanner->stack.size--;
            lexer->result_symbol = TOKEN_TEMPLATE_INTERPOLATION_END;
            return true;
        }

        if (top->type == CTX_DIRECTIVE &&
            valid_symbols[TOKEN_TEMPLATE_DIRECTIVE_END] &&
            lexer->lookahead == '}') {
            lexer->advance(lexer, false);
            lexer->mark_end(lexer);
            context_free(top);
            scanner->stack.size--;
            lexer->result_symbol = TOKEN_TEMPLATE_DIRECTIVE_END;
            return true;
        }
    }

    // ------------------------------------------------------------------
    // Skip whitespace ONLY when not inside a string/heredoc context.
    // ------------------------------------------------------------------
    if (!in_template_context(scanner)) {
        while (lexer->lookahead == ' ' || lexer->lookahead == '\t' ||
               lexer->lookahead == '\r' || lexer->lookahead == '\n') {
            lexer->advance(lexer, true);
        }
    }

    // ------------------------------------------------------------------
    // quoted_template_start: open a new quoted context
    // ------------------------------------------------------------------
    if (valid_symbols[TOKEN_QUOTED_TEMPLATE_START] && !in_template_context(scanner)) {
        if (lexer->lookahead == '"') {
            lexer->advance(lexer, false);
            lexer->mark_end(lexer);
            Context ctx;
            ctx.type = CTX_QUOTED;
            array_init(&ctx.heredoc_id);
            array_push(&scanner->stack, ctx);
            lexer->result_symbol = TOKEN_QUOTED_TEMPLATE_START;
            return true;
        }
    }

    // ------------------------------------------------------------------
    // quoted_template_end: close current quoted context
    // ------------------------------------------------------------------
    if (valid_symbols[TOKEN_QUOTED_TEMPLATE_END] && in_template_context(scanner)) {
        Context *top = &scanner->stack.contents[scanner->stack.size - 1];
        if (top->type == CTX_QUOTED && lexer->lookahead == '"') {
            lexer->advance(lexer, false);
            lexer->mark_end(lexer);
            context_free(top);
            scanner->stack.size--;
            lexer->result_symbol = TOKEN_QUOTED_TEMPLATE_END;
            return true;
        }
    }

    // ------------------------------------------------------------------
    // heredoc_identifier: scan the identifier after << or <<-.
    // Also matches the closing heredoc identifier.
    // ------------------------------------------------------------------
    if (valid_symbols[TOKEN_HEREDOC_IDENTIFIER]) {
        if (in_template_context(scanner)) {
            // We're inside a heredoc — check for the closing identifier
            Context *top = &scanner->stack.contents[scanner->stack.size - 1];
            if (top->type == CTX_HEREDOC) {
                // Skip leading whitespace (for <<- stripped heredocs)
                while (lexer->lookahead == ' ' || lexer->lookahead == '\t') {
                    lexer->advance(lexer, false);
                }
                // Try to match the heredoc identifier
                uint32_t id_len = top->heredoc_id.size;
                bool match = true;
                for (uint32_t i = 0; i < id_len; i++) {
                    if (lexer->lookahead != top->heredoc_id.contents[i]) {
                        match = false;
                        break;
                    }
                    lexer->advance(lexer, false);
                }
                if (match && (lexer->lookahead == '\n' || lexer->lookahead == '\r' ||
                              lexer->lookahead == 0)) {
                    lexer->mark_end(lexer);
                    context_free(top);
                    scanner->stack.size--;
                    lexer->result_symbol = TOKEN_HEREDOC_IDENTIFIER;
                    return true;
                }
                // Not a match — fall through to template_literal_chunk
            }
        } else {
            // Outside a heredoc — scan the opening identifier after << or <<-
            if (lexer->lookahead == '<') {
                lexer->advance(lexer, false);
                if (lexer->lookahead == '<') {
                    lexer->advance(lexer, false);
                    if (lexer->lookahead == '-') {
                        lexer->advance(lexer, false);
                    }
                    // Scan the identifier
                    if ((lexer->lookahead >= 'A' && lexer->lookahead <= 'Z') ||
                        (lexer->lookahead >= 'a' && lexer->lookahead <= 'z') ||
                        lexer->lookahead == '_') {
                        Context ctx;
                        ctx.type = CTX_HEREDOC;
                        array_init(&ctx.heredoc_id);
                        while ((lexer->lookahead >= 'A' && lexer->lookahead <= 'Z') ||
                               (lexer->lookahead >= 'a' && lexer->lookahead <= 'z') ||
                               (lexer->lookahead >= '0' && lexer->lookahead <= '9') ||
                               lexer->lookahead == '_') {
                            array_push(&ctx.heredoc_id, (char)lexer->lookahead);
                            lexer->advance(lexer, false);
                        }
                        lexer->mark_end(lexer);
                        // Consume newline after opening identifier (not part of token)
                        if (lexer->lookahead == '\n') {
                            lexer->advance(lexer, false);
                        } else if (lexer->lookahead == '\r') {
                            lexer->advance(lexer, false);
                            if (lexer->lookahead == '\n') lexer->advance(lexer, false);
                        }
                        array_push(&scanner->stack, ctx);
                        lexer->result_symbol = TOKEN_HEREDOC_IDENTIFIER;
                        return true;
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Inside a template context: handle ${, %{, and literal chunks.
    //
    // The key design: we call mark_end AFTER each character we want to
    // include in the current token. When we "peek" at the character after
    // $ or %, we advance (tentative) but only call mark_end if we decide
    // to include that character. This way, if $ is followed by {, the $
    // is NOT included in the literal chunk (the lexer rewinds to the last
    // mark_end position, which is before $).
    // ------------------------------------------------------------------
    if (in_template_context(scanner)) {
        // Track whether we tentatively consumed a character ($  or %) in the
        // pre-checks below but did NOT emit a token. If so, the literal chunk
        // handler must include that character even if the very next character
        // would otherwise end the chunk (e.g. "%" — percent is the only content).
        bool had_tentative_advance = false;

        // Check for ${  (template_interpolation_start)
        if (valid_symbols[TOKEN_TEMPLATE_INTERPOLATION_START] &&
            lexer->lookahead == '$') {
            lexer->advance(lexer, false);  // consume $ (tentative)
            if (lexer->lookahead == '{') {
                lexer->advance(lexer, false);  // consume {
                lexer->mark_end(lexer);         // commit ${ as the token
                Context ctx;
                ctx.type = CTX_INTERP;
                array_init(&ctx.heredoc_id);
                array_push(&scanner->stack, ctx);
                lexer->result_symbol = TOKEN_TEMPLATE_INTERPOLATION_START;
                return true;
            }
            // Not ${.  $ was tentatively consumed; fall through to literal chunk.
            had_tentative_advance = true;
        }

        // Check for %{  (template_directive_start)
        if (!had_tentative_advance &&
            valid_symbols[TOKEN_TEMPLATE_DIRECTIVE_START] &&
            lexer->lookahead == '%') {
            lexer->advance(lexer, false);  // consume % (tentative)
            if (lexer->lookahead == '{') {
                lexer->advance(lexer, false);  // consume {
                lexer->mark_end(lexer);
                Context ctx;
                ctx.type = CTX_DIRECTIVE;
                array_init(&ctx.heredoc_id);
                array_push(&scanner->stack, ctx);
                lexer->result_symbol = TOKEN_TEMPLATE_DIRECTIVE_START;
                return true;
            }
            // Not %{.  % was tentatively consumed; fall through to literal chunk.
            had_tentative_advance = true;
        }

        // Emit a template_literal_chunk: everything up to the next special char.
        if (valid_symbols[TOKEN_TEMPLATE_LITERAL_CHUNK]) {
            Context *top = &scanner->stack.contents[scanner->stack.size - 1];

            // If we already consumed a $ or % tentatively in the pre-checks
            // above, commit it now so it becomes part of this literal chunk
            // even if the very next character would immediately end the loop
            // (e.g. the string "%" — percent is the only content, followed by
            // the closing quote).
            bool has_content = had_tentative_advance;
            if (had_tentative_advance) {
                lexer->mark_end(lexer);
            }

            while (lexer->lookahead != 0) {
                // End of quoted string
                if (top->type == CTX_QUOTED && lexer->lookahead == '"') break;

                // Escape sequences inside quoted strings
                if (top->type == CTX_QUOTED && lexer->lookahead == '\\') {
                    has_content = true;
                    lexer->advance(lexer, false);   // consume backslash
                    if (lexer->lookahead != 0) {
                        lexer->advance(lexer, false); // consume escaped char
                    }
                    lexer->mark_end(lexer);
                    continue;
                }

                // Start of interpolation: ${ — stop the literal here.
                // Peek: advance past $ (tentative), check for {.
                if (lexer->lookahead == '$') {
                    lexer->advance(lexer, false);   // peek past $ (tentative)
                    if (lexer->lookahead == '{') {
                        // This is ${. Stop here. The $ is NOT committed
                        // (we haven't called mark_end after advancing past it),
                        // so the lexer will rewind to include $ in the next scan.
                        break;
                    }
                    if (lexer->lookahead == '$') {
                        // $$ escape: include both $ characters
                        has_content = true;
                        lexer->advance(lexer, false); // consume second $
                        lexer->mark_end(lexer);       // commit $$
                        continue;
                    }
                    // Single $ not followed by {: include it
                    has_content = true;
                    lexer->mark_end(lexer);  // commit the single $
                    continue;
                }

                // Start of directive: %{ — stop the literal here.
                if (lexer->lookahead == '%') {
                    lexer->advance(lexer, false);   // peek past % (tentative)
                    if (lexer->lookahead == '{') {
                        // This is %{. Stop here.
                        break;
                    }
                    if (lexer->lookahead == '%') {
                        // %% escape: include both
                        has_content = true;
                        lexer->advance(lexer, false);
                        lexer->mark_end(lexer);
                        continue;
                    }
                    // Single % not followed by {: include it
                    has_content = true;
                    lexer->mark_end(lexer);
                    continue;
                }

                // Newline in heredoc: yield so the parser can check for
                // the closing heredoc identifier on the next line.
                if (top->type == CTX_HEREDOC && lexer->lookahead == '\n') {
                    has_content = true;
                    lexer->advance(lexer, false);
                    lexer->mark_end(lexer);
                    break;
                }

                // Normal character
                has_content = true;
                lexer->advance(lexer, false);
                lexer->mark_end(lexer);
            }

            if (has_content) {
                // mark_end was already called at the correct position inside
                // the loop. No need to call it again.
                lexer->result_symbol = TOKEN_TEMPLATE_LITERAL_CHUNK;
                return true;
            }
        }
    }

    return false;
}
