#import <Foundation/Foundation.h>
#import <UserNotifications/UserNotifications.h>

#include "notification_bridge.h"

static UNUserNotificationCenter *vibra_notification_center(void) {
    // `currentNotificationCenter` throws if this process is not a real .app
    // bundle (`cargo run` from target/debug). Swallow that so the app still
    // launches; packaged Vibra.app delivers normally.
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
