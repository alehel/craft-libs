# craft-iwork

Reads the documents that Pages, Keynote and Numbers save (`.pages`, `.key`, `.numbers`).

- `Package` opens the ZIP package (or the folder form zipped whole, with its parts in `Index.zip`), decodes every `Index/*.iwa` part from its Snappy-compressed chunks, splits it into records, and gives each object by its ID, with its type and its payload as a schema-less Protocol Buffers `wire::Message`.
- `DataFiles` reads the files in a package's `Data/` folder, found by data ID through the package metadata.
- With the `pages` feature, `pages::import` reads a Pages document's body into a `craft_wordmodel::Document`: its paragraphs and breaks (line, page, column and section breaks), each run's character formatting (font, size, bold, italic, colour, underline, strikethrough, capitals, superscript and subscript, baseline shift, letter spacing) and links, each paragraph's formatting (alignment, indents, spacing, line spacing, tab stops, borders, drop caps), worked out from the document's styles and written as direct formatting, bookmarks, columns (a change of columns within a section starts a section of its own on the same page), pictures in the text (in the line, or floating from their place in the text) and placed on the pages (floating at their place on the page, in front of or behind the text, with their text wrap; those on a page after the first carry its number in `Float::page` and are anchored in the first paragraph, for the app to move to a paragraph on that page), the page setup (paper size, margins, header and footer distances) and the page colour.

What `pages::import` doesn't read yet is left out and counted in its `Report`, so an app can tell the user: other objects in the text and on the page (tables, shapes, text boxes, charts, page number and other fields, tables of contents, and pictures in formats the model doesn't draw), footnotes, list markers and levels, comments and highlights, headers and footers, and page colours the model can't keep (it has one for the whole document, and no gradient or picture fills). Tracked deletions are left out of the text and tracked insertions kept, as the document reads with its changes accepted. Password-protected documents are refused with `Error::PasswordProtected`, which carries the stored hint.

## Source

Written from the open iwork-format specification, built from black-box experiments: <https://alehel.github.io/iwork-format/>, source <https://github.com/alehel/iwork-format> at commit `57b31c5` (CC-BY-4.0). The sections implemented are listed in the crate documentation. Snappy is decoded from the public format description (google/snappy, `format_description.txt`) and Protocol Buffers from the public encoding description (protobuf.dev, "Encoding"). No other implementation's code or schemas were read.

## Limits

Every file is treated as hostile. `Limits` caps the ZIP entries, the bytes read and decoded (each block's size is checked before anything is allocated) and the records; style inheritance stops at 64 levels and at cycles; nothing panics. A part that is damaged partway keeps the records before the damage, and `Package::damage` lists it. The tests' hostile-input sweep feeds arbitrary bytes, arbitrary parts and damaged records and chunks to both readers.

## Tests

The tests write their own documents: records with Protocol Buffers headers, cut into chunks of Snappy literal runs and stored in a ZIP archive, as the specification describes. No file saved by the apps is committed.

Pages, Keynote and Numbers are trademarks of Apple Inc. This crate isn't affiliated with or endorsed by Apple.
