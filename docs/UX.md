# A.P.C. interaction model — design draft

The primary interface is a vertically navigated text continuum. The application is not designed around an infinite two-dimensional board.

## 1. Primary surface

Opening the application returns directly to the last working position in the active continuum.

The primary surface scrolls vertically. Broader navigation is reached deliberately, for example through a back gesture or menu action.

The user should be able to begin typing without first choosing a document type, template or editing mode.

## 2. Blocks

The visual interface presents atomic content without forcing the user to manage implementation details.

A block may represent text, a list, an attachment, a heading or future content types. Block semantics belong to the data model; decoration and presentation belong to the UI.

Creating structure should require minimal interruption. Double-tap or equivalent touch interaction may attach a structured element at the selected position, including a sticker-like block or list.

Exact gestures are platform UI decisions and are not part of the portable format.

## 3. Lists

List entry must optimize for repeated entry rather than configuration.

Adding the next item should be a direct continuation of typing. The user should not have to reopen a creation dialog for every row.

Reordering, checking, inserting and deleting items must preserve the atomic semantics required for concurrent merge.

## 4. Attachments

Attachments are embedded in the continuum rather than forcing the user into a separate external workflow.

The UI may later provide tactile visual treatment, for example an image visually pinned to the text surface and rotatable through a deliberate gesture. Such presentation must not create required visual coordinates in the portable format.

PDF and other paginated content should be readable in place, including large documents, while notes remain available immediately around that content.

## 5. Export and clipboard

Copy, paste and export are normal capabilities, not privileged emergency paths.

The application may explain when data leaves A.P.C. protection, but warnings must not turn ordinary work into a sequence of confirmations.

## 6. Security notices

Non-fatal reductions in local platform protection should appear as compact inline notices.

Example:

```text
Bootloader unlocked · hardware protection may be reduced
```

Tapping the notice dismisses it. No close icon, modal dialog or forced navigation is required.

Dismissed notices may be restored from settings.

## 7. Restraint

Security UI must not become decoration.

A.P.C. should not add decoy modes, fake applications, intrusive warning flows or other defensive ceremony unless a future threat model demonstrates a concrete requirement that cannot be met more simply.
