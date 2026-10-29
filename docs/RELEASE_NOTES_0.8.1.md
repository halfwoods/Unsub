# Unsub 0.8.1

- **Retry a damaged iPod video copy.** Add Videos now checks the content of matching videos already on the iPod before marking them as duplicates. A modified or damaged copy no longer blocks adding a fresh copy of the original, even when its file size is unchanged. Existing files stay in place.
- **Cancellable duplicate checks.** Closing the sheet, changing the selection or stopping an add operation cancels content verification. Only copies matching the selected files are read.

Includes 0.8.0's Add Videos feature, automatic playing-indicator fix and remembered window size. Physical iPod playback remains unverified. Linux support and Intel High Sierra compatibility remain experimental.

Unsub is free software under GPL-3.0-or-later. Bundled FFmpeg, LAME and Opus remain separate, replaceable libraries; their source archives are included with this release.
