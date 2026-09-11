pub(crate) fn run(options: super::BrowserKitRuntimeOptions) -> Result<(), super::Error> {
    browserkit_cef::run(browserkit_cef::Options {
        title: options.title,
        width: options.width,
        height: options.height,
        window_id: options.window_id,
        pages: options.pages,
        state: options.state,
        commands: options.commands,
        protocol_handler: options.protocol_handler,
        emit: options.emit,
    })
    .map_err(|error| super::Error::Cef(error.to_string()))
}
