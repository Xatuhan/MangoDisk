export type BatteryChargeState = 'charging' | 'discharging' | 'full' | 'paused' | 'connected' | 'unknown';
export interface BatteryCapacity {
  current: number | null;
  fullCharge: number | null;
  design: number | null;
  unit: 'milliampHours' | 'milliwattHours';
}
export interface PowerAdapter {
  inputWatts: number | null;
  reportedWatts: number | null;
  voltageVolts: number | null;
}
export interface BatteryDetails {
  chargePercent: number | null;
  externalConnected: boolean | null;
  chargeState: BatteryChargeState;
  secondsToEmpty: number | null;
  secondsToFull: number | null;
  healthPercent: number | null;
  cycleCount: number | null;
  capacity: BatteryCapacity | null;
  voltageVolts: number | null;
  currentMilliamps: number | null;
  adapter: PowerAdapter | null;
}
