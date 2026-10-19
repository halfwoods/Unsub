# Fixtures

All other test databases are synthesized by `tests/common/db.rs`; this directory holds the only file
not produced by this crate's own test code.

## libgpod-empty-iTunesDB

- Source: `bindings/python/tests/resources/iTunes/iTunesDB` in https://github.com/gtkpod/libgpod,
  last changed in commit `4f5d3cb6492ff2fcb555c0c21c8e0fd24618ce87` (2006-05-19, "Initial version").
- SHA-256: `e0f95b7f9e42d40a7ac335b3479797dde42660bec1ddf11d04cd4510fc205374`
- License: libgpod is distributed under the GNU Lesser General Public License; its source headers
  grant version 2.1 or (at your option) any later version. That is compatible with this project's
  GPL-3.0-or-later.
- Contents: 3,924 bytes, dbversion 0x0d, written by libgpod's own writer. No tracks; a master
  playlist named "Nicks iPod" (id 1234567890) and an empty Podcasts playlist.
- Not a database iTunes wrote, so it checks the reader against an independent producer but not
  against real-world iTunes quirks.
