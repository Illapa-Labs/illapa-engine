-- El último pronóstico de lluvia de Open-Meteo sobre la malla del Perú (src/forecast.rs). Una sola fila, para
-- servirlo apenas arranca la API.
create table if not exists pronostico (
    id          smallint    primary key default 1 check (id = 1),
    datos       jsonb       not null,
    actualizado timestamptz not null default now()
);
