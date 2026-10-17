//! The rules of a drop of files on a folder, like in Finder.
//!
//! The app gives the facts: the operations that the drag source offers,
//! the volumes, and the paths. This module has no AppKit code.

use std::path::{Path, PathBuf};

use crate::transfer::{TransferMode, is_within};

/// The operations that the drag source offers. AppKit applies the modifier
/// keys first: option leaves only `copy`, command leaves only `generic`,
/// and control leaves only `link`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Offered {
    pub copy: bool,
    pub link: bool,
    pub generic: bool,
    pub moves: bool,
}

/// Why a drop does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NoFiles,
    /// A move of files into the folder that has them already.
    SameFolder,
    /// A copy or a move of a folder into itself or into one of its
    /// subfolders.
    IntoItself,
}

/// Chooses the transfer mode of a drop with the Finder rules:
/// - One operation only (a modifier key): copy, alias, or move.
/// - Else a move on the same volume and a copy to another volume.
///
/// `None` when the source offers no operation that a drop can do.
pub fn choose(offered: Offered, same_volume: bool) -> Option<TransferMode> {
    let Offered {
        copy,
        link,
        generic,
        moves,
    } = offered;
    let can_move = generic || moves;
    match (copy, link, can_move) {
        (true, false, false) => Some(TransferMode::Copy),
        (false, true, false) => Some(TransferMode::Alias),
        (false, false, true) => Some(TransferMode::Move),
        _ if can_move && same_volume => Some(TransferMode::Move),
        _ if copy => Some(TransferMode::Copy),
        _ if can_move => Some(TransferMode::Move),
        _ if link => Some(TransferMode::Alias),
        _ => None,
    }
}

/// Checks a drop of `sources` into the folder `dest` in `mode`. The paths
/// must be absolute and without symlinks (`canonicalize`), so that a
/// compare of the path components is correct.
pub fn check(sources: &[PathBuf], dest: &Path, mode: TransferMode) -> Result<(), Refusal> {
    if sources.is_empty() {
        return Err(Refusal::NoFiles);
    }
    if mode != TransferMode::Alias && sources.iter().any(|s| is_within(dest, s)) {
        return Err(Refusal::IntoItself);
    }
    if mode == TransferMode::Move && sources.iter().all(|s| s.parent() == Some(dest)) {
        return Err(Refusal::SameFolder);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Offered = Offered {
        copy: true,
        link: true,
        generic: true,
        moves: true,
    };

    fn only(f: impl FnOnce(&mut Offered)) -> Offered {
        let mut offered = Offered::default();
        f(&mut offered);
        offered
    }

    #[test]
    fn choose_without_modifier_keys() {
        assert_eq!(choose(ALL, true), Some(TransferMode::Move));
        assert_eq!(choose(ALL, false), Some(TransferMode::Copy));
    }

    #[test]
    fn choose_with_one_operation() {
        // Option: copy, also on the same volume.
        assert_eq!(
            choose(only(|o| o.copy = true), true),
            Some(TransferMode::Copy)
        );
        // Control: alias.
        assert_eq!(
            choose(only(|o| o.link = true), true),
            Some(TransferMode::Alias)
        );
        // Command: move, also to another volume (the app then copies).
        assert_eq!(
            choose(only(|o| o.generic = true), false),
            Some(TransferMode::Move)
        );
        assert_eq!(choose(Offered::default(), true), None);
    }

    #[test]
    fn choose_with_a_partial_source() {
        // A source that offers copy and link: copy.
        let copy_link = only(|o| {
            o.copy = true;
            o.link = true;
        });
        assert_eq!(choose(copy_link, true), Some(TransferMode::Copy));
        // A source that offers move and link, to another volume: move.
        let move_link = only(|o| {
            o.moves = true;
            o.link = true;
        });
        assert_eq!(choose(move_link, false), Some(TransferMode::Move));
    }

    #[test]
    fn check_refuses_a_folder_into_itself() {
        let src = vec![PathBuf::from("/a/b")];
        let into_self = check(&src, Path::new("/a/b"), TransferMode::Copy);
        assert_eq!(into_self, Err(Refusal::IntoItself));
        let into_child = check(&src, Path::new("/a/b/c"), TransferMode::Move);
        assert_eq!(into_child, Err(Refusal::IntoItself));
        // A sibling with the same prefix is not a subfolder.
        assert_eq!(check(&src, Path::new("/a/bc"), TransferMode::Move), Ok(()));
        // An alias of a folder in its own subfolder is valid.
        assert_eq!(
            check(&src, Path::new("/a/b/c"), TransferMode::Alias),
            Ok(())
        );
    }

    #[test]
    fn check_a_drop_into_the_same_folder() {
        let src = vec![PathBuf::from("/a/x.txt"), PathBuf::from("/a/y.txt")];
        let dest = Path::new("/a");
        assert_eq!(
            check(&src, dest, TransferMode::Move),
            Err(Refusal::SameFolder)
        );
        // A copy makes `x copy.txt`, an alias makes `x.txt alias`.
        assert_eq!(check(&src, dest, TransferMode::Copy), Ok(()));
        assert_eq!(check(&src, dest, TransferMode::Alias), Ok(()));
        // One file from another folder: the move is valid.
        let mixed = vec![PathBuf::from("/a/x.txt"), PathBuf::from("/b/z.txt")];
        assert_eq!(check(&mixed, dest, TransferMode::Move), Ok(()));
    }

    #[test]
    fn check_without_files() {
        assert_eq!(
            check(&[], Path::new("/a"), TransferMode::Copy),
            Err(Refusal::NoFiles)
        );
    }
}
