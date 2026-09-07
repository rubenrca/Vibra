#import <AppKit/AppKit.h>
#include <stdlib.h>
#include <string.h>
#include "editor_bridge.h"

uint8_t *vibra_copy_editor_icon_png(const char *bundle_identifier, size_t *length) {
    if (length == NULL) return NULL;
    *length = 0;
    if (bundle_identifier == NULL) return NULL;
    @autoreleasepool {
        NSString *identifier = [NSString stringWithUTF8String:bundle_identifier];
        if (identifier == nil) return NULL;
        NSWorkspace *workspace = NSWorkspace.sharedWorkspace;
        NSURL *url = [workspace URLForApplicationWithBundleIdentifier:identifier];
        if (url == nil) return NULL;
        NSImage *icon = [workspace iconForFile:url.path];
        // A 32 px raster keeps the 16 pt menu icon sharp on Retina displays.
        NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc]
            initWithBitmapDataPlanes:NULL pixelsWide:32 pixelsHigh:32 bitsPerSample:8
            samplesPerPixel:4 hasAlpha:YES isPlanar:NO colorSpaceName:NSCalibratedRGBColorSpace
            bytesPerRow:0 bitsPerPixel:0];
        NSGraphicsContext *context = [NSGraphicsContext graphicsContextWithBitmapImageRep:bitmap];
        if (context == nil || icon == nil) return NULL;
        [NSGraphicsContext saveGraphicsState];
        @try {
            NSGraphicsContext.currentContext = context;
            context.imageInterpolation = NSImageInterpolationHigh;
            [icon drawInRect:NSMakeRect(0, 0, 32, 32) fromRect:NSZeroRect
                operation:NSCompositingOperationCopy fraction:1.0 respectFlipped:YES hints:nil];
        } @finally {
            [NSGraphicsContext restoreGraphicsState];
        }
        NSData *png = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
        if (png.length == 0) return NULL;
        uint8_t *bytes = malloc(png.length);
        if (bytes == NULL) return NULL;
        memcpy(bytes, png.bytes, png.length);
        *length = png.length;
        return bytes;
    }
}
