"""Genera data/zonas-pronostico.json: mallas de 0,1° (la resolución del modelo del UK Met Office) sobre las zonas con
quebradas, que se piden además de la malla de 0,2° de todo el Perú (data/malla-pronostico.json).

Solo entran las celdas en tierra (o a una celda de la costa), para no gastar consultas del límite diario de Open-Meteo
en el mar. Los contornos vienen de los departamentos de la web (peru-departamentos.geojson, SENAMHI).

Uso (Python 3 con shapely):
    python scripts/generar-zonas-pronostico.py peru-departamentos.geojson data/zonas-pronostico.json
"""
import json
import math
import sys

import shapely
from shapely.geometry import Point, shape
from shapely.ops import unary_union

STEP = 0.1
# [oeste, sur, este, norte]: múltiplos de 0,1° para que las celdas coincidan con los puntos del modelo
ZONES = [
    ("lima-este", "Lima Este: cuencas del Rímac, Chillón y Lurín (Chosica, Chaclacayo)", [-77.2, -12.6, -75.9, -11.5]),
    ("costa-norte", "Costa norte: Tumbes y Piura (El Niño costero)", [-81.0, -5.8, -79.6, -3.4]),
    ("ancash", "Áncash: Callejón de Huaylas y Cordillera Blanca", [-77.8, -10.2, -77.1, -8.7]),
    ("cusco", "Cusco: Valle Sagrado y Machu Picchu", [-73.2, -13.7, -71.9, -12.7]),
    ("arequipa", "Arequipa: quebradas del Chachani y el valle del Chili", [-71.9, -16.7, -71.2, -16.0]),
]

peru = unary_union([shapely.make_valid(shape(f["geometry"])) for f in json.load(open(sys.argv[1]))["features"]])
land = peru.buffer(STEP)  # una celda de margen: la costa también necesita dato


def tenth(value):
    return round(value, 1)


zones = []
for zone_id, name, (west, south, east, north) in ZONES:
    west, north = tenth(west), tenth(north)
    cols = round((tenth(east) - west) / STEP) + 1
    rows = round((north - tenth(south)) / STEP) + 1
    cells = [
        row * cols + col
        for row in range(rows)
        for col in range(cols)
        if land.contains(Point(west + col * STEP, north - row * STEP))
    ]
    zones.append({"id": zone_id, "nombre": name, "west": west, "north": north, "step": STEP, "cols": cols, "rows": rows, "cells": cells})
    print(f"{zone_id}: {len(cells)} puntos de {cols * rows} ({cols}x{rows})")

json.dump({"zonas": zones}, open(sys.argv[2], "w"), ensure_ascii=False, separators=(",", ":"))
print("total", sum(len(z["cells"]) for z in zones), "puntos")
