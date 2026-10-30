-- Tres nombres del libro del INEI que están abreviados, cortados o mal escritos. Se corrigen a como
-- los escribe el SENAMHI (y el uso común), para poder cruzar los avisos por nombre.
update provincias set nombre = 'CARLOS FERMIN FITZCARRALD' where ubigeo = '0207';
update distritos  set nombre = 'SANTA BARBARA DE CARHUACAYAN' where ubigeo = '120807';
update distritos  set nombre = 'ALLAUCA' where ubigeo = '151003';
