// The Windows installer size budget, shared by the release asset check and Native CI.
// Sizes are reported in MiB (1024 × 1024 bytes), the unit the budget is written in.

export const MIB = 1024 * 1024;
export const SETUP_WARN_BYTES = 5.5 * MIB;
export const SETUP_MAX_BYTES = 6 * MIB;

export const formatMiB = (bytes) => {
  return `${(bytes / MIB).toFixed(2)} MiB`;
};

/**
 * Checks one installer against the budget: a problem above 6 MiB, a warning above 5.5 MiB.
 */
export const checkSetupSize = (name, size) => {
  const line = `${name} is ${formatMiB(size)} (${size} bytes)`;
  if (size > SETUP_MAX_BYTES) {
    return { problem: `${line}, over the ${formatMiB(SETUP_MAX_BYTES)} limit` };
  }
  if (size > SETUP_WARN_BYTES) {
    return {
      warning: `${line}, over the ${formatMiB(SETUP_WARN_BYTES)} target (limit ${formatMiB(SETUP_MAX_BYTES)})`,
    };
  }

  return { note: `${line}, within the ${formatMiB(SETUP_WARN_BYTES)} target` };
};
