#import <Foundation/Foundation.h>
#import <Sparkle/Sparkle.h>

#include "sparkle_bridge.h"

static SPUStandardUpdaterController *g_controller = nil;

static bool vibra_sparkle_should_configure(void) {
    NSBundle *bundle = [NSBundle mainBundle];
    if (bundle == nil) {
        return false;
    }
    NSString *path = [bundle bundlePath];
    if (path == nil || ![path.pathExtension isEqualToString:@"app"]) {
        return false;
    }
    return [bundle objectForInfoDictionaryKey:@"SUFeedURL"] != nil;
}

void vibra_sparkle_start(void) {
    @autoreleasepool {
        if (g_controller != nil) {
            return;
        }
        if (!vibra_sparkle_should_configure()) {
            return;
        }
        // Must run on the main thread; GPUI drives NSApplication there.
        if (![NSThread isMainThread]) {
            dispatch_sync(dispatch_get_main_queue(), ^{
                vibra_sparkle_start();
            });
            return;
        }
        g_controller = [[SPUStandardUpdaterController alloc]
            initWithStartingUpdater:YES
                    updaterDelegate:nil
                 userDriverDelegate:nil];
    }
}

void vibra_sparkle_check_for_updates(void) {
    @autoreleasepool {
        if (g_controller == nil) {
            return;
        }
        if (![NSThread isMainThread]) {
            dispatch_async(dispatch_get_main_queue(), ^{
                vibra_sparkle_check_for_updates();
            });
            return;
        }
        [g_controller checkForUpdates:nil];
    }
}

bool vibra_sparkle_is_configured(void) {
    return g_controller != nil;
}
