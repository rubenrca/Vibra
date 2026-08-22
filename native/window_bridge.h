#ifndef VIBRA_WINDOW_BRIDGE_H
#define VIBRA_WINDOW_BRIDGE_H

// Starts a native macOS window drag from the mouse-down event currently being
// dispatched. This lets Vibra keep interactive controls out of the drag region.
void vibra_start_window_drag(void);

#endif
