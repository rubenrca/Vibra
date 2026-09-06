#import <AppKit/AppKit.h>

#include "window_bridge.h"

void vibra_restore_window_frame(void *view) {
    @autoreleasepool {
        NSWindow *window = ((__bridge NSView *)view).window;
        if (window == nil) {
            return;
        }

        // Keep native screen coordinates throughout, including on secondary displays.
        NSString *name = @"VibraMainWindow";
        [window setFrameUsingName:name];

        // The full display frame includes the Dock and menu bar. Fit the restored
        // window in the usable area, also handling a removed or smaller display.
        NSScreen *screen = window.screen ?: NSScreen.mainScreen;
        if (screen != nil) {
            NSRect visible = screen.visibleFrame;
            NSRect frame = window.frame;
            frame.size.width = MIN(frame.size.width, visible.size.width);
            frame.size.height = MIN(frame.size.height, visible.size.height);
            frame.origin.x = MAX(NSMinX(visible), MIN(frame.origin.x, NSMaxX(visible) - frame.size.width));
            frame.origin.y = MAX(NSMinY(visible), MIN(frame.origin.y, NSMaxY(visible) - frame.size.height));
            [window setFrame:frame display:NO];
        }

        [window saveFrameUsingName:name];
        [window setFrameAutosaveName:name];
    }
}

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
