// Evaluation-only ABI bridge; compile against the pinned upstream headers.
#include <ghostty/vt.h>
#include <assert.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    GhosttyTerminal terminal;
    GhosttyRenderState render;
} EvalTerminal;

// Do not use assert for calls with side effects: Rust release builds may set NDEBUG.
static void check(GhosttyResult result) { if (result != GHOSTTY_SUCCESS) abort(); }

void *eval_new(unsigned short cols, unsigned short rows) {
    EvalTerminal *e = calloc(1, sizeof(*e));
    if (!e) abort();
    check(ghostty_terminal_new(NULL, &e->terminal, cols, rows));
    size_t history = 1000;
    check(ghostty_terminal_set(e->terminal, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, NULL));
    check(ghostty_terminal_set(e->terminal, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &history));
    check(ghostty_render_state_new(NULL, &e->render));
    return e;
}

void eval_free(void *ptr) {
    EvalTerminal *e = ptr;
    ghostty_render_state_free(e->render);
    ghostty_terminal_free(e->terminal);
    free(e);
}

void eval_feed(void *ptr, const unsigned char *data, size_t len) {
    ghostty_terminal_vt_write(((EvalTerminal *)ptr)->terminal, data, len);
}

void eval_resize(void *ptr, unsigned short cols, unsigned short rows) {
    check(ghostty_terminal_resize(((EvalTerminal *)ptr)->terminal, cols, rows, 8, 16));
}

// Read a visible-cell fingerprint, including combining codepoints and explicit styles.
// Always walk all visible cells: this is not a benchmark of incremental drawing.
unsigned long long eval_capture(void *ptr) {
    EvalTerminal *e = ptr;
    check(ghostty_render_state_update(e->render, e->terminal));
    GhosttyRenderStateRowIterator it = NULL;
    GhosttyRenderStateRowCells cells = NULL;
    check(ghostty_render_state_row_iterator_new(NULL, &it));
    check(ghostty_render_state_row_cells_new(NULL, &cells));
    check(ghostty_render_state_get(e->render, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, &it));
    unsigned long long hash = 14695981039346656037ULL;
    while (ghostty_render_state_row_iterator_next(it)) {
        check(ghostty_render_state_row_get(it, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &cells));
        while (ghostty_render_state_row_cells_next(cells)) {
            uint32_t count = 0;
            check(ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_LEN, &count));
            uint32_t *cp = count ? malloc(count * sizeof(*cp)) : NULL;
            if (count && !cp) abort();
            if (count) check(ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_BUF, cp));
            if (!count) { hash ^= 32; hash *= 1099511628211ULL; }
            for (uint32_t i = 0; i < count; ++i) { hash ^= cp[i]; hash *= 1099511628211ULL; }
            free(cp);
            GhosttyStyle style = GHOSTTY_INIT_SIZED(GhosttyStyle);
            check(ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &style));
            hash ^= style.bold;
            hash *= 1099511628211ULL;
        }
    }
    ghostty_render_state_row_cells_free(cells);
    ghostty_render_state_row_iterator_free(it);
    check(ghostty_render_state_clean(e->render));
    return hash;
}

unsigned char *eval_format(void *ptr, size_t *len, int styled) {
    GhosttyFormatterTerminalOptions options = GHOSTTY_INIT_SIZED(GhosttyFormatterTerminalOptions);
    options.emit = styled ? GHOSTTY_FORMATTER_FORMAT_VT : GHOSTTY_FORMATTER_FORMAT_PLAIN;
    options.trim = true;
    options.extra = (GhosttyFormatterTerminalExtra)GHOSTTY_INIT_SIZED(GhosttyFormatterTerminalExtra);
    options.extra.screen = (GhosttyFormatterScreenExtra)GHOSTTY_INIT_SIZED(GhosttyFormatterScreenExtra);
    options.extra.screen.cursor = styled;
    options.extra.screen.style = styled;
    options.extra.screen.kitty_keyboard = styled;
    options.extra.modes = styled;
    options.extra.scrolling_region = styled;
    GhosttyFormatter f;
    check(ghostty_formatter_terminal_new(NULL, &f, ((EvalTerminal *)ptr)->terminal, options));
    unsigned char *out = NULL;
    check(ghostty_formatter_format_alloc(f, NULL, &out, len));
    ghostty_formatter_free(f);
    return out;
}

void eval_buffer_free(unsigned char *ptr, size_t len) { ghostty_free(NULL, ptr, len); }
