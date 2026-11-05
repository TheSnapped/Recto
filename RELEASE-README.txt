Recto 1.0.0 (Windows x64)
=========================

recto (n.): the right-hand page of an open book.

Recto is a small, quick-starting PDF viewer. It opens PDFs in a blink,
scrolls smoothly through big files, and only views: nothing in a PDF is
ever changed.


REQUIREMENTS
------------
- Windows 10 or 11, 64-bit.
- A graphics driver with OpenGL 3.1 or newer (any normal PC from the last
  decade has this).

recto.exe is all you need. It doesn't install anything and needs no admin
rights.


MAKE IT YOUR DEFAULT PDF VIEWER
-------------------------------
1. Move this folder somewhere permanent, for example C:\Tools\Recto.
2. Run recto.exe, open the menu (the three lines at the top right) and click
   "Set Recto as default PDF viewer". From a terminal,
   `recto.exe --register` does the same thing.
3. Windows opens Settings > Default apps. Choose Recto for .pdf files. On
   Windows 10, click "Choose default apps by file type", find .pdf and pick
   Recto. Windows only lets you make this choice yourself, so this click is
   needed.

To undo it, run `recto.exe --unregister`, then delete the folder. If you
move the folder later, register again.


CONTROLS
--------
  Mouse wheel, j / k    scroll            Ctrl+wheel, + / -   zoom
  Space / Shift+Space   a screen down / up (also PageDown / PageUp)
  J / K                 next / previous page
  g / G, Home / End     first / last page
  12 G  or  Ctrl+G      go to page 12 / type a page number
  Alt+Left / Alt+Right  back / forward after following a link
  0                     actual size (100 %)
  w / f                 fit width / fit page
  c                     continuous scrolling / single pages
  / or Ctrl+F           search; Enter, n or F3 = next match,
                        Shift+Enter, N or Shift+F3 = previous match
  Drag over text        select it (double-click: a word, triple: a line)
  Ctrl+C / Ctrl+A       copy / select the page's text
  i                     dark pages (inverted colours)
  t                     contents and thumbnails sidebar
  Ctrl+O / Ctrl+P       open / print
  F11                   full screen (Esc leaves)
  F1                    list of shortcuts

Drag empty space (or with the middle mouse button) to move the page. Drop
a PDF onto the window to open it. Click the page number to type one, and
the zoom percentage for fit and zoom choices.


COMMAND LINE
------------
  recto.exe document.pdf              open a file
  recto.exe document.pdf --page 42    open it at page 42

Each call opens its own window.


GOOD TO KNOW
------------
- Recto is a viewer only: no annotating, form filling, signing or saving.
  Annotations and form contents already in a PDF are shown.
- Links to web pages and e-mail addresses open in your browser or mail
  program. Links that would start other programs are copied instead of
  opened.
- Nothing is remembered about individual files. The window size and
  position, dark pages, the sidebar and the page layout are stored in
  %APPDATA%\Recto\settings.ini.
- At very high zoom on large pages, pages are rendered at no more than
  24 megapixels, so they can look slightly soft.


LICENSES
--------
Recto is open source under the GNU GPL v3 or later (see LICENSE.txt).
Source code and new releases: https://github.com/TheSnapped/Recto
Recto was built with the help of AI (Claude by Anthropic).

Recto includes MuPDF (https://mupdf.com) by Artifex Software, under the
GNU AGPL v3 (see MUPDF-LICENSE.txt). Its source code is at
https://github.com/ArtifexSoftware/mupdf. MuPDF includes FreeType,
HarfBuzz, libjpeg, OpenJPEG, jbig2dec, Little CMS, zlib, Gumbo and the URW
base fonts under their own licenses.
