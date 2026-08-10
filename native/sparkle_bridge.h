#pragma once

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

/// Start Sparkle if this process is a packaged .app with SUFeedURL configured.
/// Safe to call more than once; subsequent calls are no-ops.
void vibra_sparkle_start(void);

/// Show Sparkle's check-for-updates UI. No-op when the updater is not configured.
void vibra_sparkle_check_for_updates(void);

/// True when a packaged feed is present and the controller started.
bool vibra_sparkle_is_configured(void);

#ifdef __cplusplus
}
#endif
