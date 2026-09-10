# React BrowserKit example

Run frontend:

```bash
pnpm --filter react-browser dev --host 127.0.0.1
```

Run BrowserKit against it from another terminal:

```bash
BROWSERKIT_FRONTEND_URL=http://127.0.0.1:5173 cargo run -p compositor-smoke
```

`BrowserView` is an empty DOM slot. BrowserKit measures its CSS logical
rectangle and positions the native page surface over it. Production builds use
`vite build`; serving bundled assets inside BrowserKit remains future work.
