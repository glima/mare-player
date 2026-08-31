# Autenticación
sign-in = Iniciar sesión con TIDAL
sign-in-prompt = Inicia sesión para acceder a tu música
sign-in-title = Iniciar sesión en TIDAL
sign-out = Cerrar sesión
sign-in-button = Iniciar sesión
open-browser = Abrir navegador
cancel = Cancelar
verifying-auth = Verificando autenticación...
verifying-auth-wait = Por favor espera mientras confirmamos tu inicio de sesión.
login-step-browser = 1. Abre la página de inicio de sesión de TIDAL e inicia sesión:
login-returns-here = Al iniciar sesión volverás aquí directamente: no hay nada que copiar.
login-step-paste = 2. Después el navegador llega a una página que no carga. Copia su dirección y pégala aquí:
login-redirect-placeholder = https://tidal.com/android/login/auth?code=…
login-finish = Finalizar inicio de sesión
login-retry = Ese enlace de inicio de sesión ya se usó o ha caducado: abre el navegador otra vez y copia la nueva dirección.
preparing-login = Preparando inicio de sesión...

# Navegación
back = Atrás
search = Buscar
settings = Ajustes

# Estados de carga
loading = Cargando...
loading-albums = Cargando álbumes...
loading-artist = Cargando artista...
loading-mixes = Cargando mixes...
loading-playlists = Cargando listas de reproducción...
loading-tracks = Cargando pistas...
loading-radio-tracks = Cargando pistas de radio...
loading-recommendations = Cargando…
loading-followed-artists = Cargando artistas seguidos...
searching = Buscando...

# Estados vacíos
no-albums-found = No se encontraron álbumes
no-mixes-found = No se encontraron mixes
no-playlists-found = No se encontraron listas de reproducción
no-tracks-album = No hay pistas en este álbum
no-tracks-mix = No hay pistas en este mix
no-tracks-playlist = No hay pistas en esta lista de reproducción
no-radio-tracks = No se encontraron pistas de radio
no-results = No se encontraron resultados
no-favorite-tracks = No se encontraron pistas favoritas
no-followed-artists = No hay artistas seguidos
no-history = Aún no hay historial de reproducción
enter-search-term = Introduce un término de búsqueda
not-signed-in = Sesión no iniciada

# Vista principal
collection = Colección
history = Historial
feed = Novedades
explore = Explorar
mixes-and-radio = Mixes y Radio
no-feed = Sin actividad en el feed
no-explore = Nada que explorar por ahora
feed-new-updates = Nuevas actualizaciones
feed-last-week = Última semana
feed-last-month = Último mes
feed-older = Más antiguo

# Encabezados de sección
albums = Álbumes
artists = Artistas
playlists = Listas de reproducción
videos = Vídeos
profiles = Perfiles
tracks = Pistas
favorite-tracks = Pistas favoritas

# Detalle del artista
top-tracks = Pistas populares
discography = Discografía
popularity = Popularidad: {$value}
artist-top-tracks-context = {$artist} — Pistas populares

# Detalle de la pista
more-albums-by = Más álbumes de {$artist}
related-albums = Álbumes relacionados
related-artists = Artistas relacionados

# Radio de la pista
track-radio = Radio de {$title}
track-radio-fallback = Radio de la pista

# Etiquetas de respaldo
fallback-track = Pista
fallback-artist = Artista
fallback-album = Álbum
fallback-mix = Mix
fallback-playlist = Lista de reproducción

# Metadatos
released = Lanzamiento: {$year}
quality-label = Calidad: {$quality}
track-count = {$count} {$count ->
    [one] pista
   *[other] pistas
}
artist-count = {$count} {$count ->
    [one] artista
   *[other] artistas
}

# Búsqueda
search-placeholder = Buscar pistas, álbumes, artistas...

# Depuración
debug-unoptimized = (sin optimizar)

# Ajustes
audio-quality = Calidad de audio
quality-description-low = Menor consumo de datos
quality-description-high = Con pérdida, pero consume pocos datos
quality-description-lossless = Calidad CD
quality-description-hires = Requiere un plan que incluya streaming de alta resolución
account = Cuenta
about = Acerca de
version = Versión
explicit = Explícito

# Historial
clear-history = Borrar historial
history-filter-placeholder = Filtrar historial…
favorite-tracks-filter-placeholder = Filtrar pistas favoritas…

# Información emergente
tooltip-search = Buscar
tooltip-settings = Ajustes
tooltip-shuffle-play = Reproducción aleatoria
tooltip-refresh = Actualizar
tooltip-previous-track = Pista anterior
tooltip-next-track = Pista siguiente
tooltip-pause = Pausar
tooltip-play = Reproducir
tooltip-stop = Detener
tooltip-go-to-track-radio = Ir a la radio de la pista
tooltip-share = Compartir
tooltip-video-popout = Abrir el vídeo en una ventana aparte
tooltip-video-inline = Mostrar el vídeo integrado
tooltip-add-to-favorites = Añadir a favoritos
tooltip-remove-from-favorites = Quitar de favoritos
tooltip-follow-artist = Seguir artista
tooltip-unfollow-artist = Dejar de seguir artista
tooltip-enable-shuffle = Activar reproducción aleatoria
tooltip-disable-shuffle = Desactivar reproducción aleatoria
tooltip-mode-normal = Modo de reproducción: Normal (clic para cambiar)
tooltip-mode-shuffle = Modo de reproducción: Aleatorio (clic para cambiar)
tooltip-mode-repeat-all = Modo de reproducción: Repetir todo (clic para cambiar)
tooltip-mode-repeat-track = Modo de reproducción: Repetir pista (clic para cambiar)
tooltip-volume = Volumen: {$percent}%

# Botones
refresh = Actualizar

# Compartir
share = Compartir
share-description = Generar una URL de song.link y copiar al portapapeles
share-track = Compartir pista: {$title}
share-album = Compartir álbum: {$title}

# Etiquetas de contexto (usadas para el contexto de la cola de reproducción)
context-favorites = Favoritos
context-history = Historial
context-search = Búsqueda

# Lyrics
lyrics-title = Letra — {$title}
lyrics-title-fallback = Letra
loading-lyrics = Cargando letra…
no-lyrics-available = No hay letra disponible para «{$title}».
lyrics-provider = Letra de {$provider}

# Créditos
credits-title = Créditos — {$title}
credits-title-fallback = Créditos
loading-credits = Cargando créditos…
no-credits-available = No hay créditos disponibles para «{$title}».
credits-field-title = TÍTULO
credits-field-artists = ARTISTAS
credits-field-album = ÁLBUM
credits-field-released = LANZAMIENTO
credits-field-label = SELLO
credits-field-isrc = ISRC
credits-field-bpm = BPM
tooltip-show-lyrics = Mostrar letra
tooltip-show-credits = Mostrar créditos
tooltip-back = Atrás (clic derecho: inicio)
