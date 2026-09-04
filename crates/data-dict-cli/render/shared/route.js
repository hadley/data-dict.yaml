/* Hash routing, shared by both pages: the hash drives the page, so the back
   button and a pasted link both work. Each page keeps its own parseHash —
   the dictionary splits on "." for #table.column, the report on "/" for
   #view/key — and everything here works off whatever it returns. */

function go(hash) {
  if (location.hash === hash) dispatchEvent(new HashChangeEvent("hashchange"));
  else location.hash = hash;
}

/* Back to the index, leaving no `#` behind in the address bar. */
function goHome() {
  if (location.hash) history.replaceState(null, "", location.pathname + location.search);
  dispatchEvent(new HashChangeEvent("hashchange"));
}

function useRoute() {
  const [route, setRoute] = useState(parseHash);
  useEffect(() => {
    /* A tooltip belongs to what you were pointing at, and following a link out
       of the diagram hides that without the pointer ever leaving it. */
    const follow = () => {
      hideTip();
      setRoute(parseHash());
    };
    addEventListener("hashchange", follow);
    return () => removeEventListener("hashchange", follow);
  }, []);
  return route;
}
