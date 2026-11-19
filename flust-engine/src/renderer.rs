/// The renderer the engine draws frames with.
///
/// Both draw through the OpenGL callbacks of the embedder. They differ in
/// what they need from the framebuffers they draw into, so an embedder has
/// to know which one the engine uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Renderer {
    /// The engine's original renderer.
    #[default]
    Skia,

    /// Flutter's newer renderer. Through the embedder API (OpenGL), the
    /// engine draws with Impeller's OpenGL ES backend.
    Impeller,
}

impl Renderer {
    /// The engine switch that selects Impeller.
    const ENABLE_IMPELLER: &'static str = "--enable-impeller";

    /// The renderer the engine will use when it is started with `args`, if
    /// they say: the last `--enable-impeller` switch among them decides, the
    /// way the engine reads it (no value or `true` for Impeller, any other
    /// value for Skia).
    pub fn from_args<S: AsRef<str>>(args: &[S]) -> Option<Self> {
        args.iter().rev().find_map(|arg| {
            let value = arg.as_ref().strip_prefix(Self::ENABLE_IMPELLER)?;
            match value {
                "" | "=true" => Some(Self::Impeller),
                value if value.starts_with('=') => Some(Self::Skia),
                // Another switch that starts with the same name.
                _ => None,
            }
        })
    }

    /// The renderer `args` select, or `default` if they don't.
    pub fn resolve<S: AsRef<str>>(default: Self, args: &[S]) -> Self {
        Self::from_args(args).unwrap_or(default)
    }

    /// The switch that has the engine use this renderer.
    pub(crate) fn engine_switch(self) -> String {
        match self {
            Self::Skia => format!("{}=false", Self::ENABLE_IMPELLER),
            Self::Impeller => format!("{}=true", Self::ENABLE_IMPELLER),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Renderer;

    #[test]
    fn from_args() {
        let none: [&str; 0] = [];
        assert_eq!(Renderer::from_args(&none), None);
        assert_eq!(Renderer::from_args(&["--verbose-logging"]), None);
        assert_eq!(
            Renderer::from_args(&["--enable-impeller"]),
            Some(Renderer::Impeller)
        );
        assert_eq!(
            Renderer::from_args(&["--enable-impeller=true"]),
            Some(Renderer::Impeller)
        );
        assert_eq!(
            Renderer::from_args(&["--enable-impeller=false"]),
            Some(Renderer::Skia)
        );
        assert_eq!(
            Renderer::from_args(&["--enable-impeller=1"]),
            Some(Renderer::Skia)
        );
        assert_eq!(
            Renderer::from_args(&["--enable-impeller", "--enable-impeller=false"]),
            Some(Renderer::Skia)
        );
        assert_eq!(Renderer::from_args(&["--enable-impeller-foo"]), None);
    }

    #[test]
    fn resolve() {
        let none: [&str; 0] = [];
        assert_eq!(
            Renderer::resolve(Renderer::Impeller, &none),
            Renderer::Impeller
        );
        assert_eq!(
            Renderer::resolve(Renderer::Impeller, &["--enable-impeller=false"]),
            Renderer::Skia
        );
        assert_eq!(
            Renderer::resolve(Renderer::Skia, &["--enable-impeller"]),
            Renderer::Impeller
        );
    }
}
