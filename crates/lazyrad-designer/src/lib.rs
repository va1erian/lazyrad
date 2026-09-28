#![forbid(unsafe_code)]

//! The LazyRAD form-designer surface.
//!
//! It renders the form's real xui widgets in design mode and lays a transparent
//! `Custom` overlay on top for the dot grid, selection handles, alignment
//! guides and mouse editing. The model lives in [`lazyrad_project`] and the
//! toolbox and property grid sit alongside the surface. See PLAN.md §6.
