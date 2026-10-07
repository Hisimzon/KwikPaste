# Changelog

All notable changes to KwikPaste are documented here.

## 2.0.0-beta.1 - 2026-10-07

- The first beta of KwikPaste 2.0. The whole app is rewritten with a native Rust interface and no longer uses a WebView, so it uses less memory and opens faster.
- You can install it over 1.x; your records, settings and images carry over.
- This is a beta and may still have problems. Please report anything you run into.

## 1.4.0 - 2026-10-02

- Added Export to files in Preference › Data › Backup & Migration. It exports records to Excel or Markdown for reading, sorting and sharing. You can export only favorites or chosen groups, as one file or one file per group; sensitive records are left out by default. Exported files are not encrypted and cannot restore your data, so keep using Export Backup for full backups.
- Added in-app announcements. When KwikPaste has important news, such as a new release, it shows a system dialog during the update check, at most once a day. Choose Don't remind me to hide that announcement for good. With Automatically Check for Updates turned off, KwikPaste no longer fetches announcements at launch.
- New installs now start with Space Preview off and Text Preview Style set to Words. Existing settings stay as they are.
- Fixed a single copy occasionally creating two identical records.
- Fixed the clipboard window on Windows sometimes appearing behind other always-on-top windows.

## 1.3.8 - 2026-09-29

- Added multi-select. Click Select multiple at the bottom of the clipboard window, or choose Select Multiple from a record's context menu, to pick several records and delete them at once. Click to select, hold Shift to select a range, and press Ctrl+A (⌘A on macOS) to select all. Favorite and pinned records still follow the delete protection settings.
- Added list display settings in Preference › Appearance. List Style switches between Cards and Seamless, where records sit edge to edge with dividers. List Density offers Comfortable, Standard, Compact and Custom; Custom lets you set Record Spacing, Vertical Padding and whether to Show Header Row.
- Added Tray Icon Click Opens on Windows: choose whether clicking the tray icon opens the clipboard window or Preferences. It sits under System Tray Icon in Preference › General and still opens the clipboard window by default.
- Simplified Preference: setting rows no longer carry icons, descriptions appear only where they help, and Overview has its own page in the sidebar.
- On macOS you can now install KwikPaste with Homebrew: `brew install --cask mansandadada/tap/kwikpaste`.
- Fixed text dragged out on Windows not dropping into WPS and other apps.
- Fixed the Preference window not shrinking back to 960×600 after raising the Windows text size.

## 1.3.7 - 2026-09-28

- Reworked Auto Cleanup in Preference › Data. Records now expire by when you last used them instead of when they were first copied, so something copied long ago that you still paste every day is kept. Copying or pasting from history counts as use even with Update Record on Reuse turned off.
- Added custom cleanup rules. Give text, images, files, sensitive content, records from certain apps, records over a given size, or records copied only once their own retention, from minutes to forever. Rules match from top to bottom, and records that match no rule follow the Default Retention. Presets cover common cases such as images over 5 MB and sensitive content.
- Saving a cleanup setting that would delete records right away now shows how many would go and why, and asks first. A new Cleanup Status row shows the last cleanup and has a Clean Up Now button.
- Automatic cleanup now also keeps records in a custom group and records with a note, as it already did for favorites and pinned records.
- Cleanup settings take effect right away instead of at the next launch, and the Cleanup Interval setting is gone. Maximum Items applies as soon as new records come in, and the storage limit no longer rescans the data folder every minute.
- The clipboard window's footer now warns when storage is over the limit, either in Remind only mode or when auto cleanup can't free enough space. Click the warning to open the storage settings.
- The database file now shrinks after records are cleaned up. For existing data, run Clean Cache once in Preference › Data to compact the database and turn this on.
- Added Open with Mouse Button on Windows: click the middle button or a side button to open or hide the clipboard window. It is off by default; turn it on in Preference › Shortcuts.
- Reorganized Preference into nine categories. Each category shows all its settings on one scrolling page, with links to its sections at the top.
- Fixed keys pressed in the note, group and confirmation dialogs, drop-down menus or the search box also reaching the list, such as Enter pasting the selected item or Esc hiding the window.
- Fixed Ctrl+V and other Ctrl edits in the Windows search box going to the app you were in before, and arrow keys leaving the search box.
- Fixed the note and group name inputs not taking focus when their dialogs open on Windows.

## 1.3.6 - 2026-09-26

- Added Data Overview in Preference › Data. It shows what takes up storage, the total record count, daily additions, content types and source apps, and can clear all records of one content type or from one app at once while keeping favorites and pinned items. Click the storage usage at the bottom left to open it.
- Fixed the clipboard window opening on the primary display instead of the one under the cursor on Windows setups with several displays and display scaling turned on.
- Fixed the clipboard, Preference, update, context menu, preview and onboarding windows cutting off their content after raising the Windows Text size setting. These windows now grow with it, and onboarding steps that do not fit can be scrolled.
- Fixed pinned items showing as a solid block with the Mica or Acrylic window material.
- Fixed a User Account Control prompt appearing at every login when Run as Administrator and Launch at Login were both on. Starting with administrator privileges on a laptop running on battery no longer fails.
- Fixed shortcuts, the tray icon and Launch at Login still following the old settings after importing a backup.
- Fixed drop-down options in Preference being cut off, such as the Quick Paste modifier keys showing only "Ctr...".

## 1.3.5 - 2026-09-25

- Fixed pasting an image copied from a browser or another app adding a duplicate record every time. Pasting large images is also faster, taking about 40% of the time it used to for a 2–3 megapixel image.
- Fixed a newly copied image staying a grey placeholder, or showing the previous image, when the clipboard window was already open.
- Hovering, selecting and scrolling in the clipboard list now redraw only the cards that changed instead of every visible card.
- Paging through history, the category tabs, custom groups and history cleanup now use database indexes, so they stay fast as history grows. The first launch after updating builds the indexes once, which takes about a second with 20,000 records.
- Copying no longer extracts the source app's icon every time; each app's icon is read once.
- App Info now has a Website link to the official site, and the GitHub repository moved to its own Source Code link.

## 1.3.2 - 2026-09-24

- Added Quick Paste: hold the modifier keys and press a number to paste without opening the clipboard window. 1–9 paste items 1–9 and 0 pastes item 10, in the same order as the All list. It is off by default; turn it on in Preference › Shortcuts, where the modifier keys (Ctrl+Shift by default) can be changed.
- Text records now list the codes, model numbers, numbers and links found in them below each item. Click one to paste just that part. This is on by default and can be turned off with Extract Quick Info.
- Added Split Words: split a text record into words, pick the ones you need, and paste only those.
- The hover preview stays open while the pointer moves onto it, and its text can be shown as words to pick from and paste in place.
- Hover preview is now on by default for new installs. Existing settings are kept.
- The clipboard window now opens on All by default for new installs. Existing settings are kept.
- Opening Preference now hides the clipboard window.
- Added a Windows portable build: unzip it and run. Data stays in the `data` folder next to the app, and in-app updates replace the portable app in place.
- Shrank the Windows installer from 7.6 MB to 4.2 MB.

## 1.3.0 - 2026-09-24

- Added a Storage Limit setting, 1 GB by default. The storage meter in the sidebar fills at this size. Going over it only shows a reminder by default; switch to Auto clean to delete the oldest regular records until usage is back under the limit. Favorites and pinned records are always kept.
- Moved Clear Records from the clipboard window's more-actions menu into the Storage Locations settings.
- Trimmed the tray menu to Preference and Exit, and removed the version number from the tray tooltip.
- Update checks now use the system proxy, query every enabled channel at the same time, and time out instead of hanging when a server can't be reached.

## 1.2.0 - 2026-09-23

- Turned on in-app updates. Update packages are signed with the KwikPaste key and verified before installing, and downloads come from a mainland China CDN with GitHub as the fallback.
- Automatic update checks are now on by default, including beta and nightly builds. When several channels are enabled, KwikPaste installs whichever one offers the newest version.
- Unified window materials and appearance controls, using native materials where the system supports them and falling back where it doesn't.
- Turned the hover preview into a native-material panel window.
- Load image thumbnails on demand, with same-size placeholders while they load.
- Stopped the hover preview from redoing window setup and data loading on every hover frame.
- Avoided redundant database reads and full-text index rewrites.
- Fixed hover preview panel positioning and opacity.
- Named the app and installers KwikPaste on both Windows and macOS.
- Removed the first-run legacy data import step.

## 1.1.0 - 2026-09-16

- Established the Chinese product name **快贴** and the English technical name **KwikPaste**.
- Changed the application identifier to `com.fastthree.kwikpaste`.
- Replaced application, installer, favicon, and tray artwork with the KwikPaste K mark.
- Added `.kwikpastebak` backup packages and isolated `KwikPasteData` storage.
- Disabled the inherited updater until FastThree signing and release infrastructure are ready.
- Removed the inherited sponsor QR entry and redirected the project link to `https://paste.fastthree.com`.

Earlier upstream release history is available in the [EcoPaste](https://github.com/EcoPasteHub/EcoPaste) repository. Required attribution is retained in README.md and LICENSE.
