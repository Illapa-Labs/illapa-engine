-- Qué avisos del SENAMHI tocan a cada distrito. Lo llena el refresco en segundo plano (src/refresh.rs).
--   nivel_lluvia:    nivel más alto del aviso de lluvia (pronóstico a 24 h) que toca al distrito
--   nivel_quebradas: nivel más alto del aviso de activación de quebradas
--   Los niveles son los del SENAMHI: 0 sin aviso, 2 amarillo, 3 naranja, 4 rojo.
--   centros_poblados_* y personas_*: centros poblados del distrito dentro de zonas del aviso de
--   activación de quebradas, y sus habitantes, por nivel.
create table if not exists distrito_aviso (
    ubigeo                    char(6)  primary key references distritos (ubigeo),
    nivel_lluvia              smallint not null default 0,
    nivel_quebradas           smallint not null default 0,
    centros_poblados_amarillo integer  not null default 0,
    centros_poblados_naranja  integer  not null default 0,
    centros_poblados_rojo     integer  not null default 0,
    personas_amarillo         integer  not null default 0,
    personas_naranja          integer  not null default 0,
    personas_rojo             integer  not null default 0
);

-- El último refresco: el aviso de lluvia ya aligerado para el mapa, la fecha del aviso de
-- quebradas y cuándo se bajó. Una sola fila, para servir datos apenas arranca la API.
create table if not exists refresco (
    id              smallint    primary key default 1 check (id = 1),
    aviso           jsonb       not null,
    fecha_quebradas text,
    actualizado     timestamptz not null default now()
);
