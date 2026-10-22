---
name: maplibre-nextjs
description: Best practices, setup guide, component patterns, and troubleshooting for MapLibre GL in React and Next.js applications (App Router, Turbopack, SSR safety, raster/vector tile styling, headless fallbacks, and layer management). Use when building interactive maps, GIS telemetry, GPS tracking, vehicle markers, routes, and fixing map rendering errors in React/Next.js.
argument-hint: "[map-feature or issue]"
license: MIT
metadata:
  author: p063-team
  version: "1.0.0"
---

# MapLibre GL in Next.js & React Best Practices

Comprehensive guide for integrating and debugging MapLibre GL in Next.js App Router (Turbopack/Webpack) and React applications.

## 1. Core Architecture & SSR Boundaries

MapLibre GL requires the browser `window`, `document`, and a valid WebGL context (`canvas.getContext('webgl')`). In Next.js App Router:
- Always mark the component file with `"use client";`.
- Dynamically import MapLibre GL inside `useEffect` or use `next/dynamic` with `{ ssr: false }`.
- Test WebGL support explicitly before initialization to prevent crashes in headless environments (e.g. Vitest/Jest, JSDOM, Playwright without GPU).

```tsx
"use client";

import React, { useEffect, useRef, useState } from "react";

export function isWebGLSupported(): boolean {
  if (typeof window === "undefined" || typeof document === "undefined") {
    return false;
  }
  try {
    const canvas = document.createElement("canvas");
    const gl =
      canvas.getContext("webgl") ||
      canvas.getContext("experimental-webgl") ||
      canvas.getContext("webgl2");
    return Boolean(gl);
  } catch {
    return false;
  }
}
```

## 2. CSS Styling & Container Sizing

MapLibre canvas must have explicit dimensions (`width`, `height`) and the stylesheet imported:
1. In `globals.css` or component:
   ```css
   @import "maplibre-gl/dist/maplibre-gl.css";
   ```
2. In the container:
   ```tsx
   <div ref={mapContainerRef} className="w-full h-64 relative overflow-hidden" />
   ```
   If the container starts with `height: 0` or hidden in tabs, call `map.resize()` when it becomes visible.

## 3. High-Reliability Tile Providers

Public `tile.openstreetmap.org` blocks automated user agents, headless browsers, or high-volume IP requests (returning HTTP 502/429). Always use reliable raster or vector tile mirrors:

```typescript
// CartoDB Voyager Raster Tiles (No API key needed, high reliability, CORS enabled)
const cartoRasterStyle: any = {
  version: 8,
  sources: {
    carto: {
      type: "raster",
      tiles: [
        "https://basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}@2x.png",
        "https://a.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}@2x.png",
        "https://b.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}@2x.png",
        "https://c.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}@2x.png",
      ],
      tileSize: 256,
      attribution: '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors &copy; <a href="https://carto.com/attributions">CARTO</a>',
    },
  },
  layers: [
    {
      id: "carto-tiles",
      type: "raster",
      source: "carto",
      minzoom: 0,
      maxzoom: 20,
    },
  ],
};
```

## 4. Map Lifecycle, Layers & GeoJSON Sources

1. **Initialization:**
   ```typescript
   const maplibregl = await import("maplibre-gl");
   const map = new maplibregl.Map({
     container: mapContainerRef.current,
     style: cartoRasterStyle,
     center: [lng, lat],
     zoom: 15.5,
   });
   ```
2. **Wait for `load` event:** Never add sources or layers before the map has fired the `"load"` event.
3. **GeoJSON updates:** When coordinates update, use `map.getSource(id).setData(newGeoJSON)` instead of re-creating the layer.
4. **Cleanup:**
   ```typescript
   return () => {
     markers.forEach(m => m.remove());
     map.remove();
   };
   ```

## 5. Custom Markers with HTML/Tailwind Elements

```typescript
const el = document.createElement("div");
el.className = "flex flex-col items-center select-none";
el.innerHTML = `
  <div class="bg-slate-900 text-white text-[9px] px-1.5 py-0.5 rounded shadow font-mono">
    🚗 Active
  </div>
`;

const marker = new maplibregl.Marker({ element: el })
  .setLngLat([lng, lat])
  .addTo(map);
```

## 6. Headless Test Fallback Pattern

In automated test suites (Vitest, Jest) where WebGL is unavailable:
- Conditionally render a fallback SVG vector map with street names and breadcrumbs if `!hasWebGL`.
- All DOM elements, layer toggle buttons, and test locators (`data-testid="maplibre-view"`, `data-testid="driver-vehicle-marker"`) remain present and testable.
