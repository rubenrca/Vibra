#import <Foundation/Foundation.h>
#import <UserNotifications/UserNotifications.h>

#include "notification_bridge.h"

void vibra_notification_request_authorization(void) {
    @autoreleasepool {
        UNUserNotificationCenter *center = [UNUserNotificationCenter currentNotificationCenter];
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

        UNUserNotificationCenter *center = [UNUserNotificationCenter currentNotificationCenter];
        [center requestAuthorizationWithOptions:(UNAuthorizationOptionAlert | UNAuthorizationOptionSound)
                              completionHandler:^(BOOL granted, NSError *error) {
                                  (void)error;
                                  if (!granted) {
                                      return;
                                  }
                                  UNMutableNotificationContent *content =
                                      [UNMutableNotificationContent new];
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
                              }];
    }
}
