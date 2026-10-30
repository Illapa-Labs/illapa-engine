-- El libro del INEI escribe «San Pedro de Putina Puncu»; el SENAMHI (y el uso común) «Putina Punco».
-- Se corrige para poder cruzar el aviso de activación de quebradas, que solo trae nombres.
update distritos set nombre = 'SAN PEDRO DE PUTINA PUNCO' where ubigeo = '211210';
