#pragma once
#include <stddef.h>
#include <stdint.h>
typedef void (*VgEvent)(void *, int, const uint8_t *, size_t);
typedef struct {
    uint16_t columns, rows, cursor_x, cursor_y;
    uint8_t cursor_visible, cursor_blinking, cursor_style, kitty;
    uint64_t history, offset;
    uint32_t modes, painted_cells;
} VgInfo;
typedef struct {
    uint8_t foreground[3], background[3], underline_color[3];
    uint8_t bold, italic, underline, strikeout, hidden, wide_spacer, selected;
} VgCell;
typedef void (*VgPaint)(void *, uint16_t, uint16_t, const VgCell *, const uint8_t *, size_t, const uint8_t *, size_t);
void *vg_new(uint16_t, uint16_t, VgEvent, void *);
void vg_free(void *);
void vg_feed(void *, const uint8_t *, size_t);
int vg_resize(void *, uint16_t, uint16_t, uint32_t, uint32_t);
int vg_palette(void *, const uint8_t *);
int vg_snapshot(void *, VgInfo *, VgPaint, void *, int);
void vg_scroll(void *, int64_t);
int vg_clear_history(void *);
int vg_select(void *, int, int, uint16_t, uint16_t, int);
int vg_search(void *, const uint8_t *, size_t, int);
uint8_t *vg_text(void *, size_t *, int);
void vg_buffer_free(uint8_t *, size_t);

uint8_t *vg_recent_text(void *, size_t *, size_t);
uint8_t *vg_remote_row(void *, size_t *, uint16_t);
int vg_remote_info(void *, VgInfo *);
int vg_remote_palette(void *, uint8_t *);
