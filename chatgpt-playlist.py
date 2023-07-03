import openai
import spotipy
from spotipy.oauth2 import SpotifyOAuth
import sys

def get_pop_songs(api_key):
    openai.api_key = api_key

    response = openai.Completion.create(
        engine='text-davinci-003',
        prompt='Generate a list of popular pop songs:',
        max_tokens=100,
        temperature=0.7,
        n=10,
        stop=None
    )

    pop_songs = [choice['text'].strip() for choice in response['choices']]
    [ print(song) for song in pop_songs ]
    exit(0)
    return pop_songs

def create_spotify_playlist(sp, playlist_name, track_uris):
    user_id = sp.me()['id']
    playlist = sp.user_playlist_create(user_id, playlist_name, public=False)

    playlist_id = playlist['id']
    sp.playlist_add_items(playlist_id, track_uris)

    print("Spotify playlist created successfully!")

def main():
    if len(sys.argv) < 4:
        print("Usage: python chatgpt-playlist.py YOUR_API_KEY YOUR_CLIENT_ID YOUR_CLIENT_SECRET")
        exit(-1)

    api_key = sys.argv[1]
    spotify_client_id = sys.argv[2]
    spotify_client_secret = sys.argv[3]

    songs = get_pop_songs(api_key)

    track_uris = []
    sp = spotipy.Spotify(auth_manager=SpotifyOAuth(client_id=spotify_client_id,
                                                   client_secret=spotify_client_secret,
                                                   redirect_uri='http://localhost:3000',
                                                   scope='playlist-modify-private'))

    for song in songs:
        results = sp.search(q=song, type='track', limit=1)
        items = results['tracks']['items']
        if items:
            track_uris.append(items[0]['uri'])

    #create_spotify_playlist(sp, "ChatGPT Playlist", track_uris)

if __name__ == '__main__':
    main()

