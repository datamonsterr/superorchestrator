import type { Register } from 'claude-code';

// The monitor's hooks arrive with the rendering task. The module is here so
// the plugin loads as a mod and its model can be tested on its own.
export const register: Register = () => {};
