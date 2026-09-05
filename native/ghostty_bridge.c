// The unstable upstream ABI stays inside this translation unit. Rust only sees
// the small fixed-layout ABI in ghostty_bridge.h. All access is serialized there.
#include "ghostty_bridge.h"
#include <ghostty/vt.h>
#include <stdlib.h>
#include <string.h>
_Static_assert(sizeof(GhosttyColorRgb) == 3, "RGB ABI must match the Rust bridge");
#define TRY(expr) do { GhosttyResult r_ = (expr); if (r_ != GHOSTTY_SUCCESS) return (int)r_; } while (0)
typedef struct {
    GhosttyTerminal term;
    GhosttyRenderState render;
    GhosttySelectionGesture gesture;
    GhosttySearch search;
    VgEvent event;
    void *userdata;
    bool rectangle;
    bool capture_reply;
    uint8_t reply[256];
    size_t reply_len;
} VgTerminal;
static void write_pty(GhosttyTerminal t, void *u, const uint8_t *p, size_t n) {
    (void)t; VgTerminal *v = u;
    if (v->capture_reply) {
        if (n <= sizeof(v->reply) - v->reply_len) {
            memcpy(v->reply + v->reply_len, p, n); v->reply_len += n;
        }
        return;
    }
    v->event(v->userdata, 0, p, n);
}
static void bell(GhosttyTerminal t, void *u) {
    (void)t; VgTerminal *v = u; v->event(v->userdata, 1, NULL, 0);
}
static void title(GhosttyTerminal t, void *u) {
    VgTerminal *v = u; GhosttyString s = {0};
    if (ghostty_terminal_get(t, GHOSTTY_TERMINAL_DATA_TITLE, &s) == GHOSTTY_SUCCESS)
        v->event(v->userdata, 2, s.ptr, s.len);
}
static void clipboard_write(GhosttyTerminal t, void *u, const GhosttyClipboardWrite *write) {
    (void)t; VgTerminal *v=u;
    GhosttyClipboardWriteReply reply=GHOSTTY_INIT_SIZED(GhosttyClipboardWriteReply);
    reply.result=GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
    if (write->contents_len==0) {
        v->event(v->userdata,3,NULL,0); reply.result=GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
    }
    for (size_t i=0;i<write->contents_len;++i) {
        GhosttyClipboardContent c=write->contents[i];
        if (c.mime.len>=10 && memcmp(c.mime.ptr,"text/plain",10)==0) {
            v->event(v->userdata,3,c.data.ptr,c.data.len);
            reply.result=GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS; break;
        }
    }
    write->reply(write,&reply);
}
// Render a denial while the borrowed request is valid, but retain its bytes.
// OSC 52's empty response gives us the exact destination and terminator. Rust
// owns that template and fills it only after the existing UI grants consent.
// No borrowed request survives this callback and the VT parser never blocks.
static void clipboard_read(GhosttyTerminal t, void *u, const GhosttyClipboardRead *read) {
    (void)t; VgTerminal *v = u;
    GhosttyClipboardReadReply reply = GHOSTTY_INIT_SIZED(GhosttyClipboardReadReply);
    reply.result = GHOSTTY_CLIPBOARD_READ_RESULT_DENIED;
    v->reply_len = 0; v->capture_reply = true;
    read->reply(read, &reply);
    v->capture_reply = false;
    bool osc52 = v->reply_len >= 7 && memcmp(v->reply, "\x1b]52;", 5) == 0;
    // Other clipboard protocols retain their explicit denial; no data is read.
    v->event(v->userdata, osc52 ? 4 : 0, v->reply, v->reply_len);
}
void vg_free(void *p) {
    VgTerminal *v = p; if (!v) return;
    if (v->search) ghostty_search_free(v->search);
    if (v->gesture) ghostty_selection_gesture_free(v->gesture, v->term);
    if (v->render) ghostty_render_state_free(v->render);
    if (v->term) ghostty_terminal_free(v->term);
    free(v);
}
void *vg_new(uint16_t cols, uint16_t rows, VgEvent event, void *userdata) {
    VgTerminal *v = calloc(1, sizeof(*v)); if (!v) return NULL;
    v->event = event; v->userdata = userdata;
    if (ghostty_terminal_new(NULL, &v->term, cols, rows) != GHOSTTY_SUCCESS ||
        ghostty_render_state_new(NULL, &v->render) != GHOSTTY_SUCCESS ||
        ghostty_selection_gesture_new(NULL, &v->gesture) != GHOSTTY_SUCCESS ||
        ghostty_search_new(NULL, &v->search, v->term) != GHOSTTY_SUCCESS) goto fail;
    if (ghostty_terminal_set(v->term, GHOSTTY_TERMINAL_OPT_USERDATA, v) != GHOSTTY_SUCCESS ||
        ghostty_terminal_set(v->term, GHOSTTY_TERMINAL_OPT_WRITE_PTY, write_pty) != GHOSTTY_SUCCESS ||
        ghostty_terminal_set(v->term, GHOSTTY_TERMINAL_OPT_BELL, bell) != GHOSTTY_SUCCESS ||
        ghostty_terminal_set(v->term, GHOSTTY_TERMINAL_OPT_TITLE_CHANGED, title) != GHOSTTY_SUCCESS) goto fail;
    if (ghostty_terminal_set(v->term,GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,clipboard_write)!=GHOSTTY_SUCCESS) goto fail;
    if (ghostty_terminal_set(v->term,GHOSTTY_TERMINAL_OPT_CLIPBOARD_READ,clipboard_read)!=GHOSTTY_SUCCESS) goto fail;
    GhosttyString name = {(const uint8_t *)"xterm-256color", 14};
    if (ghostty_terminal_set(v->term, GHOSTTY_TERMINAL_OPT_TERMINFO_NAME, &name) != GHOSTTY_SUCCESS) goto fail;
    size_t history_bytes=64u*1024u*1024u, history_lines=10000, no_images=0;
    if (ghostty_terminal_set(v->term,GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,&history_bytes)!=GHOSTTY_SUCCESS ||
        ghostty_terminal_set(v->term,GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,&history_lines)!=GHOSTTY_SUCCESS ||
        ghostty_terminal_set(v->term,GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,&no_images)!=GHOSTTY_SUCCESS) goto fail;
    return v;
fail: vg_free(v); return NULL;
}
void vg_feed(void *p, const uint8_t *s, size_t n) { ghostty_terminal_vt_write(((VgTerminal *)p)->term, s, n); }
int vg_resize(void *p, uint16_t c, uint16_t r, uint32_t w, uint32_t h) {
    return ghostty_terminal_resize(((VgTerminal *)p)->term, c, r, w, h);
}
int vg_palette(void *p, const uint8_t *rgb) {
    VgTerminal *v = p;
    TRY(ghostty_terminal_set(v->term, GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, rgb));
    TRY(ghostty_terminal_set(v->term, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, rgb + 256*3));
    TRY(ghostty_terminal_set(v->term, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, rgb + 257*3));
    TRY(ghostty_terminal_set(v->term, GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, rgb + 258*3));
    return 0;
}
static bool mode(VgTerminal *v, uint16_t n) {
    GhosttyTerminalModeConfig c = {ghostty_mode_new(n, false), false};
    return ghostty_terminal_get(v->term, GHOSTTY_TERMINAL_DATA_MODE, &c) == GHOSTTY_SUCCESS && c.value;
}
int vg_snapshot(void *p, VgInfo *info, VgPaint paint, void *userdata, int force) {
    VgTerminal *v = p;
    info->painted_cells = 0;
    TRY(ghostty_render_state_update(v->render, v->term));
    TRY(ghostty_terminal_get(v->term, GHOSTTY_TERMINAL_DATA_COLS, &info->columns));
    TRY(ghostty_terminal_get(v->term, GHOSTTY_TERMINAL_DATA_ROWS, &info->rows));
    GhosttyTerminalScrollbar bar = {0};
    TRY(ghostty_terminal_get(v->term, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &bar));
    info->history = bar.total > bar.len ? bar.total - bar.len : 0;
    info->offset = info->history > bar.offset ? info->history - bar.offset : 0;
    GhosttyRenderStateCursor cursor = GHOSTTY_INIT_SIZED(GhosttyRenderStateCursor);
    TRY(ghostty_render_state_get(v->render, GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor));
    info->cursor_visible = cursor.viewport_has_value && cursor.visible;
    info->cursor_x = cursor.viewport_has_value ? cursor.viewport_x : 0;
    info->cursor_y = cursor.viewport_has_value ? cursor.viewport_y : 0;
    info->cursor_style = (uint8_t)cursor.visual_style; info->cursor_blinking = cursor.blinking;
    TRY(ghostty_terminal_get(v->term, GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS, &info->kitty));
    const uint16_t modes[] = {1,2004,1049,1007,1004,1000,1002,1003,1006,1005};
    info->modes = 0;
    for (size_t i = 0; i < sizeof(modes)/sizeof(*modes); ++i) if (mode(v,modes[i])) info->modes |= 1u << i;
    GhosttyTerminalScreen screen;
    TRY(ghostty_terminal_get(v->term, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &screen));
    if (screen != GHOSTTY_TERMINAL_SCREEN_PRIMARY) info->modes |= 1u << 2;
    if (mode(v,9)) info->modes |= 1u << 5;
    if (!paint) return 0;
    GhosttyRenderStateColors colors = GHOSTTY_INIT_SIZED(GhosttyRenderStateColors);
    TRY(ghostty_render_state_get(v->render, GHOSTTY_RENDER_STATE_DATA_COLORS, &colors));
    GhosttyRenderStateRowIterator it = NULL;
    GhosttyRenderStateRowCells cells = NULL;
    TRY(ghostty_render_state_row_iterator_new(NULL, &it));
    GhosttyResult result = ghostty_render_state_row_cells_new(NULL, &cells);
    if (result != GHOSTTY_SUCCESS) { ghostty_render_state_row_iterator_free(it); return result; }
    result = ghostty_render_state_get(v->render, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, &it);
    bool reverse = mode(v,5);
    GhosttyRenderStateDirty dirty = GHOSTTY_RENDER_STATE_DIRTY_FALSE;
    ghostty_render_state_get(v->render, GHOSTTY_RENDER_STATE_DATA_DIRTY, &dirty);
    uint16_t y = 0;
    while (result == GHOSTTY_SUCCESS && ghostty_render_state_row_iterator_next(it)) {
        bool row_dirty = false;
        result = ghostty_render_state_row_get(it, GHOSTTY_RENDER_STATE_ROW_DATA_DIRTY, &row_dirty);
        if (result != GHOSTTY_SUCCESS) break;
        if (!force && dirty != GHOSTTY_RENDER_STATE_DIRTY_FULL && !row_dirty) { ++y; continue; }
        result = ghostty_render_state_row_get(it, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &cells);
        uint16_t x = 0;
        while (result == GHOSTTY_SUCCESS && ghostty_render_state_row_cells_next(cells)) {
            VgCell out = {0}; GhosttyStyle style = GHOSTTY_INIT_SIZED(GhosttyStyle);
            result = ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &style);
            if (result != GHOSTTY_SUCCESS) break;
            GhosttyColorRgb fg = colors.foreground, bg = colors.background;
            ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, &fg);
            ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, &bg);
            if (style.bold && style.fg_color.tag == GHOSTTY_STYLE_COLOR_PALETTE && style.fg_color.value.palette < 8)
                fg = colors.palette[style.fg_color.value.palette + 8];
            if (style.faint) { fg.r = fg.r * 66 / 100; fg.g = fg.g * 66 / 100; fg.b = fg.b * 66 / 100; }
            if (style.inverse != reverse) { GhosttyColorRgb tmp = fg; fg = bg; bg = tmp; }
            GhosttyColorRgb ul = fg;
            if (style.underline_color.tag == GHOSTTY_STYLE_COLOR_RGB) ul = style.underline_color.value.rgb;
            if (style.underline_color.tag == GHOSTTY_STYLE_COLOR_PALETTE) ul = colors.palette[style.underline_color.value.palette];
            memcpy(out.foreground, &fg, 3); memcpy(out.background, &bg, 3); memcpy(out.underline_color, &ul, 3);
            out.bold=style.bold; out.italic=style.italic; out.underline=(uint8_t)style.underline;
            out.strikeout=style.strikethrough; out.hidden=style.invisible;
            bool selected = false;
            ghostty_render_state_row_cells_get(cells,GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED,&selected); out.selected=selected;
            GhosttyCell raw = 0; GhosttyCellWide wide = GHOSTTY_CELL_WIDE_NARROW; bool link = false;
            ghostty_render_state_row_cells_get(cells,GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW,&raw);
            ghostty_cell_get(raw,GHOSTTY_CELL_DATA_WIDE,&wide);
            ghostty_cell_get(raw,GHOSTTY_CELL_DATA_HAS_HYPERLINK,&link);
            out.wide_spacer = wide == GHOSTTY_CELL_WIDE_SPACER_HEAD || wide == GHOSTTY_CELL_WIDE_SPACER_TAIL;
            uint8_t stack[128]; GhosttyBuffer text = {.ptr=stack,.cap=sizeof(stack)};
            result = ghostty_render_state_row_cells_get(cells,GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,&text);
            uint8_t *heap = NULL, *uri = NULL; size_t uri_len = 0;
            if (result == GHOSTTY_OUT_OF_SPACE) {
                heap = malloc(text.len); if (!heap) { result=GHOSTTY_OUT_OF_MEMORY; break; }
                text.ptr=heap; text.cap=text.len;
                result=ghostty_render_state_row_cells_get(cells,GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8,&text);
            }
            if (link) {
                GhosttyPoint point = {.tag=GHOSTTY_POINT_TAG_VIEWPORT,.value={.coordinate={.x=x,.y=y}}};
                GhosttyGridRef ref = GHOSTTY_INIT_SIZED(GhosttyGridRef);
                if (ghostty_terminal_grid_ref(v->term,point,&ref)==GHOSTTY_SUCCESS &&
                    ghostty_grid_ref_hyperlink_uri(&ref,NULL,0,&uri_len)==GHOSTTY_OUT_OF_SPACE) {
                    uri=malloc(uri_len);
                    if (uri) ghostty_grid_ref_hyperlink_uri(&ref,uri,uri_len,&uri_len);
                    else { uri_len=0; result=GHOSTTY_OUT_OF_MEMORY; }
                }
            }
            if (result == GHOSTTY_SUCCESS) { paint(userdata,x,y,&out,text.ptr,text.len,uri,uri_len); ++info->painted_cells; }
            free(uri); free(heap); ++x;
        }
        ++y;
    }
    ghostty_render_state_row_cells_free(cells); ghostty_render_state_row_iterator_free(it);
    if (result == GHOSTTY_SUCCESS) result=ghostty_render_state_clean(v->render);
    return result;
}
void vg_scroll(void *p, int64_t delta) {
    ghostty_terminal_scroll_viewport(((VgTerminal *)p)->term,(GhosttyTerminalScrollViewport){.tag=GHOSTTY_SCROLL_VIEWPORT_DELTA,.value={.delta=delta}});
}
int vg_clear_history(void *p) {
    VgTerminal *v=p; size_t zero=0, limit=64u*1024u*1024u;
    TRY(ghostty_terminal_set(v->term,GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,&zero));
    return ghostty_terminal_set(v->term,GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,&limit);
}
int vg_select(void *p, int action, int kind, uint16_t x, uint16_t y, int right) {
    VgTerminal *v=p;
    if (action==0) {
        ghostty_selection_gesture_reset(v->gesture,v->term);
        return ghostty_terminal_set(v->term,GHOSTTY_TERMINAL_OPT_SELECTION,NULL);
    }
    if (action==1) { ghostty_selection_gesture_reset(v->gesture,v->term); v->rectangle=kind==1; }
    GhosttyPoint point={.tag=GHOSTTY_POINT_TAG_VIEWPORT,.value={.coordinate={.x=x,.y=y}}};
    GhosttyGridRef ref=GHOSTTY_INIT_SIZED(GhosttyGridRef);
    TRY(ghostty_terminal_grid_ref(v->term,point,&ref));
    GhosttySelectionGestureEvent e=NULL;
    TRY(ghostty_selection_gesture_event_new(NULL,&e,action==1?GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_PRESS:GHOSTTY_SELECTION_GESTURE_EVENT_TYPE_DRAG));
    GhosttySurfacePosition pos={.x=x*10+(right?8:2),.y=y*10+5};
    uint16_t cols=0,rows=0;
    ghostty_terminal_get(v->term,GHOSTTY_TERMINAL_DATA_COLS,&cols);
    ghostty_terminal_get(v->term,GHOSTTY_TERMINAL_DATA_ROWS,&rows);
    GhosttySelectionGestureGeometry geometry={.columns=cols,.cell_width=10,.screen_height=rows*10};
    GhosttySelectionGestureBehaviors behaviors={.single_click=kind==2?GHOSTTY_SELECTION_GESTURE_BEHAVIOR_WORD:kind==3?GHOSTTY_SELECTION_GESTURE_BEHAVIOR_LINE:GHOSTTY_SELECTION_GESTURE_BEHAVIOR_CELL};
    ghostty_selection_gesture_event_set(e,GHOSTTY_SELECTION_GESTURE_EVENT_OPT_REF,&ref);
    ghostty_selection_gesture_event_set(e,GHOSTTY_SELECTION_GESTURE_EVENT_OPT_POSITION,&pos);
    ghostty_selection_gesture_event_set(e,GHOSTTY_SELECTION_GESTURE_EVENT_OPT_GEOMETRY,&geometry);
    ghostty_selection_gesture_event_set(e,GHOSTTY_SELECTION_GESTURE_EVENT_OPT_RECTANGLE,&v->rectangle);
    ghostty_selection_gesture_event_set(e,GHOSTTY_SELECTION_GESTURE_EVENT_OPT_BEHAVIORS,&behaviors);
    GhosttySelection selection=GHOSTTY_INIT_SIZED(GhosttySelection);
    GhosttyResult result=ghostty_selection_gesture_event(v->gesture,v->term,e,&selection);
    ghostty_selection_gesture_event_free(e);
    if (result==GHOSTTY_SUCCESS) return ghostty_terminal_set(v->term,GHOSTTY_TERMINAL_OPT_SELECTION,&selection);
    if (result==GHOSTTY_NO_VALUE) return action==1?ghostty_terminal_set(v->term,GHOSTTY_TERMINAL_OPT_SELECTION,NULL):0;
    return result;
}
int vg_search(void *p,const uint8_t *s,size_t n,int previous) {
    VgTerminal *v=p; GhosttyString needle={s,n};
    if (!n) { vg_select(p,0,0,0,0,0); return 0; }
    TRY(ghostty_search_set(v->search,GHOSTTY_SEARCH_OPT_NEEDLE,&needle));
    TRY(ghostty_search_run(v->search));
    GhosttyTerminalScrollbar original = {0};
    TRY(ghostty_terminal_get(v->term,GHOSTTY_TERMINAL_DATA_SCROLLBAR,&original));
    size_t total = 0;
    TRY(ghostty_search_get(v->search,GHOSTTY_SEARCH_DATA_TOTAL_MATCHES,&total));
    // Upstream indexes ASCII case-insensitively. Filter each candidate against
    // its exact text to preserve Vibra's literal, case-sensitive search.
    for (size_t i = 0; i < total; ++i) {
        TRY(ghostty_search_set(v->search,previous?GHOSTTY_SEARCH_OPT_SELECT_NEXT:GHOSTTY_SEARCH_OPT_SELECT_PREV,NULL));
        GhosttySelection selected=GHOSTTY_INIT_SIZED(GhosttySelection);
        TRY(ghostty_search_get(v->search,GHOSTTY_SEARCH_DATA_SELECTED_MATCH,&selected));
        GhosttyTerminalSelectionFormatOptions options=GHOSTTY_INIT_SIZED(GhosttyTerminalSelectionFormatOptions);
        options.unwrap=true; options.selection=&selected;
        uint8_t *text=NULL; size_t len=0;
        TRY(ghostty_terminal_selection_format_alloc(v->term,NULL,options,&text,&len));
        bool exact=len==n && memcmp(text,s,n)==0;
        ghostty_free(NULL,text,len);
        if (!exact) continue;
        TRY(ghostty_terminal_set(v->term,GHOSTTY_TERMINAL_OPT_SELECTION,&selected));
        return 1;
    }
    ghostty_terminal_scroll_viewport(v->term,(GhosttyTerminalScrollViewport){.tag=GHOSTTY_SCROLL_VIEWPORT_ROW,.value={.row=original.offset}});
    return 0;
}

uint8_t *vg_text(void *p,size_t *len,int selection) {
    VgTerminal *v=p; uint8_t *out=NULL; *len=0;
    if (selection) {
        GhosttyTerminalSelectionFormatOptions options=GHOSTTY_INIT_SIZED(GhosttyTerminalSelectionFormatOptions);
        options.trim=true; options.unwrap=true;
        if (ghostty_terminal_selection_format_alloc(v->term,NULL,options,&out,len)!=GHOSTTY_SUCCESS) return NULL;
    } else {
        GhosttyFormatterTerminalOptions options=GHOSTTY_INIT_SIZED(GhosttyFormatterTerminalOptions);
        options.emit=GHOSTTY_FORMATTER_FORMAT_PLAIN; options.trim=true;
        GhosttyFormatter f=NULL;
        if (ghostty_formatter_terminal_new(NULL,&f,v->term,options)!=GHOSTTY_SUCCESS) return NULL;
        GhosttyResult r=ghostty_formatter_format_alloc(f,NULL,&out,len); ghostty_formatter_free(f);
        if (r!=GHOSTTY_SUCCESS) return NULL;
    }
    return out;
}
void vg_buffer_free(uint8_t *p,size_t n) { ghostty_free(NULL,p,n); }

uint8_t *vg_recent_text(void *p, size_t *len, size_t lines) {
    VgTerminal *v=p; size_t total=0; uint16_t cols=0; *len=0;
    if (ghostty_terminal_get(v->term,GHOSTTY_TERMINAL_DATA_TOTAL_ROWS,&total)!=GHOSTTY_SUCCESS ||
        ghostty_terminal_get(v->term,GHOSTTY_TERMINAL_DATA_COLS,&cols)!=GHOSTTY_SUCCESS || !total || !cols) return NULL;
    GhosttySelection selection=GHOSTTY_INIT_SIZED(GhosttySelection);
    GhosttyPoint first={.tag=GHOSTTY_POINT_TAG_SCREEN,.value={.coordinate={.x=0,.y=(uint32_t)(total>lines?total-lines:0)}}};
    GhosttyPoint last={.tag=GHOSTTY_POINT_TAG_SCREEN,.value={.coordinate={.x=cols-1,.y=(uint32_t)(total-1)}}};
    if (ghostty_terminal_grid_ref(v->term,first,&selection.start)!=GHOSTTY_SUCCESS ||
        ghostty_terminal_grid_ref(v->term,last,&selection.end)!=GHOSTTY_SUCCESS) return NULL;
    GhosttyTerminalSelectionFormatOptions options=GHOSTTY_INIT_SIZED(GhosttyTerminalSelectionFormatOptions);
    options.trim=true; options.selection=&selection;
    uint8_t *out=NULL;
    if (ghostty_terminal_selection_format_alloc(v->term,NULL,options,&out,len)!=GHOSTTY_SUCCESS) return NULL;
    return out;
}
