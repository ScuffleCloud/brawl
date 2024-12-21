/// Copied from the std because its currently nightly only
pub fn format_fn<F>(f: F) -> FormatFn<F>
where
    F: Fn(&mut std::fmt::Formatter) -> std::fmt::Result,
{
    FormatFn(f)
}

pub struct FormatFn<F>(F);

impl<F> std::fmt::Display for FormatFn<F>
where
    F: Fn(&mut std::fmt::Formatter) -> std::fmt::Result,
{
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.0(f)
    }
}
