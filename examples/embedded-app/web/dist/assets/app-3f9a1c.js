const echo = await ipc.invoke("echo", "embedded");
const route = await fetch("/settings/profile");
const routeBody = await route.text();
const missing = await fetch("/missing.png");
const self = await fetch("/assets/app-3f9a1c.js");
document.getElementById("out").textContent = `Served from ${location.href}`;
ipc.emit("report", {
  href: location.href,
  echo,
  route_status: route.status,
  route_is_index: routeBody.includes("<h1>Embedded App</h1>"),
  missing_status: missing.status,
  asset_cache: self.headers.get("Cache-Control") || "",
});
