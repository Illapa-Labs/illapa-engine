-- Los últimos avisos de inundación de ríos del SENAMHI (SONICS-PBI, src/floods.rs): el resumen con lo expuesto por
-- distrito, el mapa de cada día (estado actual y 7 días de pronóstico) y la firma de la corrida (niveles y fechas),
-- para saber cuándo el SENAMHI publica una nueva. Una sola fila, para servirlos apenas arranca la API.
create table if not exists inundaciones (
    id          smallint    primary key default 1 check (id = 1),
    resumen     jsonb       not null,
    dias        jsonb       not null,
    firma       text        not null,
    actualizado timestamptz not null default now()
);
