# Unsub 0.6.0

- New **Gaslight** visualizer, inspired by the classic iTunes plug-in: mirrored spectrum ridges, glowing plumes and fading trails that respond to your music. Choose it from **View → Visualizer → Gaslight**.
- Choose Ember, Aurora or Inverted colors, adjust trails, glow and height, or enable slow autopilot in **Preferences → Playback**. Settings are saved between launches. Gaslight also works through software rendering when WebGL2 is unavailable.
- Fullscreen visualizer controls retain keyboard focus, so arrow keys and Escape keep working after entering or leaving fullscreen.
- The standard **Intel Mac download** now includes experimental compatibility with **macOS High Sierra 10.13.6 and Safari 13.1.2**. No separate legacy edition is needed. Native API fallbacks, older-WebKit layouts and bundled audio libraries target that baseline.

**High Sierra status:** the Intel app and its libraries pass build and binary compatibility checks, but have not been tested on an actual High Sierra Mac. Launch, playback, graphics, iTunes import and updates on that OS remain unverified. Please treat this as experimental support. Apple Silicon still requires macOS 11 or later; Windows requirements are unchanged.

Gaslight is an original built-in renderer inspired by Jonathan del Strother's [GasLight](https://steelskies.com/gaslight/). The original iTunes plug-in is not included or required.

Mac downloads are signed and notarized. Windows installers have signed updater packages but do not yet have an Authenticode certificate. Linux continues to pass CI but has no packaged download.

Unsub remains free software under the GNU GPL v3 or later. Bundled FFmpeg, libmp3lame and libopus sources accompany the release; see `scripts/THIRD_PARTY.md` for licensing details.
