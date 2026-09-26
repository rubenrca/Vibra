#import <Foundation/Foundation.h>
#import <Security/Security.h>
#import <LocalAuthentication/LocalAuthentication.h>
#include <stdlib.h>
#include <string.h>

// The caller owns the returned buffer. Automatic polls never display Keychain UI;
// only an explicit refresh may ask macOS to authorize access to the CLI's item.
uint8_t *vibra_copy_usage_credential(const char *service, const char *account,
                                    bool interactive, size_t *length, int32_t *status) {
    *length = 0;
    *status = errSecParam;
    @autoreleasepool {
        NSString *serviceName = [NSString stringWithUTF8String:service];
        if (serviceName == nil) return NULL;
        LAContext *context = [[LAContext alloc] init];
        context.interactionNotAllowed = !interactive;
        NSMutableDictionary *query = [@{
            (__bridge id)kSecClass: (__bridge id)kSecClassGenericPassword,
            (__bridge id)kSecAttrService: serviceName,
            (__bridge id)kSecReturnData: @YES,
            (__bridge id)kSecMatchLimit: (__bridge id)kSecMatchLimitOne,
            (__bridge id)kSecUseAuthenticationContext: context,
        } mutableCopy];
        if (account != NULL) {
            NSString *accountName = [NSString stringWithUTF8String:account];
            if (accountName == nil) return NULL;
            query[(__bridge id)kSecAttrAccount] = accountName;
        }
        CFTypeRef result = NULL;
        *status = SecItemCopyMatching((__bridge CFDictionaryRef)query, &result);
        if (*status != errSecSuccess || result == NULL) return NULL;
        NSData *data = CFBridgingRelease(result);
        if (![data isKindOfClass:NSData.class] || data.length == 0 || data.length > 1048576) {
            *status = errSecDecode;
            return NULL;
        }
        uint8_t *bytes = malloc(data.length);
        if (bytes == NULL) { *status = errSecAllocate; return NULL; }
        memcpy(bytes, data.bytes, data.length);
        *length = data.length;
        return bytes;
    }
}
