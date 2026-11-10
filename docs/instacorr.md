# InstaCorr

InstaCorr computes a seed-to-surface correlation map from an active surface
time-series overlay.

## Start a session

1. Load a surface and a surface time-series overlay, and make the overlay active.
2. Right-click a vertex to select a seed.
3. Press **D** to open the separate InstaCorr controller window.
4. Check **TR (seconds)**, polynomial order, and bandpass settings.
5. Click **Recalculate** to apply the settings.

With the session active, ordinary right-clicks update the seed using the applied
settings. **Shift-right-click** moves the crosshair without recalculating.
**Update seed** recomputes the result for the current vertex using the cached
preprocessing.

**D** toggles the session. Closing the controller window leaves the session
active; use **Active** or **D** to suspend updates. The result remains an overlay.

## Preprocessing

With **SUMA preprocessing** enabled, Sumaru detrends, optionally bandpass filters,
and normalizes the time series before computing correlations. Turning it off
computes a raw dot product and ignores the dependent preprocessing settings.

Settings are drafts until you click **Recalculate**. Moving the seed alone uses
the previously applied settings.

## Short or rapidly sampled recordings

TR is the sample interval in seconds. At 300 Hz, enter approximately
`0.0033333333`, not `300`.

The default 0.01–0.1 Hz bandpass is unsuitable for a roughly 1.2-second recording:
its first nonzero FFT bin is around 0.83 Hz, so the filter removes all usable
frequencies. Disable **Bandpass**, keep **SUMA preprocessing** enabled, and click
**Recalculate**, or select a passband appropriate to your analysis.

## Invalid seed time series

This message means the selected series cannot be normalized after preprocessing.
Possible causes include missing/nonfinite data, a constant signal, or settings
that remove all signal variation. Check the raw trace, time units, and filter
settings.

Current builds normalize independently of the signal's physical amplitude, so
small MEG-scale values alone should not cause this error.
