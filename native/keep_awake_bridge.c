#include <IOKit/pwr_mgt/IOPMLib.h>

#include "keep_awake_bridge.h"

int32_t vibra_keep_awake_enable(uint32_t *assertion_id) {
    // Keeping the display awake also prevents idle system sleep, on AC or battery.
    return IOPMAssertionCreateWithName(kIOPMAssertPreventUserIdleDisplaySleep,
                                     kIOPMAssertionLevelOn,
                                     CFSTR("Vibra Keep Awake"), assertion_id);
}

int32_t vibra_keep_awake_disable(uint32_t assertion_id) {
    return IOPMAssertionRelease(assertion_id);
}
