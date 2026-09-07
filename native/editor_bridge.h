#ifndef VIBRA_EDITOR_BRIDGE_H
#define VIBRA_EDITOR_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

// Call on the main thread. The caller frees the returned PNG buffer with free().
uint8_t *vibra_copy_editor_icon_png(const char *bundle_identifier, size_t *length);

#endif
