#pragma once

#ifdef __cplusplus
extern "C" {
#endif

/// Ask the user for notification permission if it has not been decided yet.
void vibra_notification_request_authorization(void);

/// Play the lightweight cue used when an agent finishes in another foreground pane.
void vibra_notification_play_completion_sound(void);

/// Deliver a local notification. Replaces any previous request with the same identifier.
void vibra_notification_deliver(const char *title, const char *body, const char *identifier);

#ifdef __cplusplus
}
#endif
