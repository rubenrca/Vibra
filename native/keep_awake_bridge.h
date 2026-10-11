#ifndef VIBRA_KEEP_AWAKE_BRIDGE_H
#define VIBRA_KEEP_AWAKE_BRIDGE_H

#include <stdint.h>

int32_t vibra_keep_awake_enable(uint32_t *assertion_id);
int32_t vibra_keep_awake_disable(uint32_t assertion_id);

#endif
