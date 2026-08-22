#import <AppKit/AppKit.h>

#include "window_bridge.h"

void vibra_start_window_drag(void) {
    @autoreleasepool {
        NSEvent *event = NSApp.currentEvent;
        if (event == nil || event.type != NSEventTypeLeftMouseDown) {
            return;
        }

        NSWindow *window = event.window ?: NSApp.keyWindow;
        if (window == nil) {
            return;
        }

        [window performWindowDragWithEvent:event];
    }
}
