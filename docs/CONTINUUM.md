# A.P.C. Continuum semantics

Continuum is the requirement that the application resumes the user's work rather than merely reopening the application.

## 1. Default launch behavior

On normal launch, the application should open the last active continuum at the last committed working position.

A library, continuum picker or general menu is secondary navigation and must not replace continuation as the default launch path.

## 2. Restorable state

The platform implementation must durably retain enough state to reconstruct the working context. Depending on editor state, this may include:

- active continuum identifier;
- active block or structural anchor;
- viewport/scroll anchor and offset;
- cursor or selection position;
- expanded/collapsed structural state where that state materially affects the working position;
- currently opened attachment page or equivalent local position;
- pending editor mode required to reproduce what the user was doing.

The portable content format and local Continuum session state are distinct. Session state may be platform-local unless a later feature explicitly synchronizes it.

## 3. Commit boundary

A user-visible edit must not be acknowledged as saved while its only authoritative copy remains in volatile process memory.

The implementation must define a durability boundary. After that boundary is crossed, immediate process termination or device power loss must not revert the acknowledged change.

Durability may be implemented with transactional storage, journaling/WAL or another mechanism, but the mechanism must satisfy the observable requirement rather than merely enqueue a background save.

## 4. View-state durability

Continuum restoration is not limited to document contents.

The last stable working location should also survive unexpected termination. High-frequency UI state may use controlled coalescing where writing every pixel of scrolling would be wasteful, but recovery must remain close enough to the last settled user position that the user does not have to find their place again.

Content durability and view-state durability may use different commit policies.

## 5. Navigation

A back gesture or explicit navigation control may leave the current continuum and expose broader navigation.

Opening the application itself must not force the user through that navigation hierarchy when a valid previous working context exists.

## 6. Failure cases

If the previous working context cannot be reconstructed because the referenced content was deleted, corrupted or unavailable, the application should recover to the nearest valid structural position without altering user data merely to satisfy UI restoration.

Continuum state is never an excuse to invent missing content.
