#import <AppKit/AppKit.h>
#import <UserNotifications/UserNotifications.h>

#include "notification_bridge.h"

static BOOL vibra_is_packaged_app(void) {
    NSString *path = [[NSBundle mainBundle] bundlePath];
    return [[path pathExtension] caseInsensitiveCompare:@"app"] == NSOrderedSame;
}

static UNUserNotificationCenter *vibra_notification_center(void) {
    // `currentNotificationCenter` throws NSInternalInconsistencyException when
    // this process is not a real .app (`cargo run` from target/debug). Skip
    // the API entirely in that case; packaged Vibra.app delivers normally.
    if (!vibra_is_packaged_app()) {
        return nil;
    }
    @try {
        return [UNUserNotificationCenter currentNotificationCenter];
    } @catch (NSException *exception) {
        NSLog(@"Vibra notifications unavailable: %@", exception.reason);
        return nil;
    }
}

void vibra_notification_request_authorization(void) {
    @autoreleasepool {
        UNUserNotificationCenter *center = vibra_notification_center();
        if (center == nil) {
            return;
        }
        [center requestAuthorizationWithOptions:(UNAuthorizationOptionAlert | UNAuthorizationOptionSound)
                              completionHandler:^(BOOL granted, NSError *error) {
                                  (void)granted;
                                  (void)error;
                              }];
    }
}

void vibra_notification_play_completion_sound(void) {
    dispatch_async(dispatch_get_main_queue(), ^{
        // Glass is short and distinct without sounding like an error. Fall back
        // to the user's configured alert sound if it is unavailable.
        NSSound *sound = [NSSound soundNamed:@"Glass"];
        if (sound != nil) {
            [sound play];
        } else {
            NSBeep();
        }
    });
}

void vibra_notification_deliver(const char *title, const char *body, const char *identifier) {
    @autoreleasepool {
        if (title == NULL || body == NULL) {
            return;
        }
        NSString *nsTitle = [NSString stringWithUTF8String:title];
        NSString *nsBody = [NSString stringWithUTF8String:body];
        NSString *nsId = identifier != NULL ? [NSString stringWithUTF8String:identifier]
                                            : [[NSUUID UUID] UUIDString];
        if (nsTitle == nil || nsBody == nil || nsId == nil) {
            return;
        }

        UNUserNotificationCenter *center = vibra_notification_center();
        if (center == nil) {
            return;
        }
        [center getNotificationSettingsWithCompletionHandler:^(UNNotificationSettings *settings) {
            void (^post)(void) = ^{
                UNMutableNotificationContent *content = [UNMutableNotificationContent new];
                content.title = nsTitle;
                content.body = nsBody;
                content.sound = [UNNotificationSound defaultSound];
                UNNotificationRequest *request =
                    [UNNotificationRequest requestWithIdentifier:nsId
                                                         content:content
                                                         trigger:nil];
                [center addNotificationRequest:request
                         withCompletionHandler:^(NSError *addError) {
                             (void)addError;
                         }];
            };
            UNAuthorizationStatus status = settings.authorizationStatus;
            if (status == UNAuthorizationStatusAuthorized ||
                status == UNAuthorizationStatusProvisional) {
                post();
            } else if (status == UNAuthorizationStatusNotDetermined) {
                [center requestAuthorizationWithOptions:(UNAuthorizationOptionAlert |
                                                         UNAuthorizationOptionSound)
                                      completionHandler:^(BOOL granted, NSError *error) {
                                          (void)error;
                                          if (granted) {
                                              post();
                                          }
                                      }];
            }
        }];
    }
}
