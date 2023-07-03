#
# PROOF OF CONCEPT. NOT ROBUST, SECURE OR EVEN WELL WRITTEN.
# THIS CODE IS MESSY BECAUSE I'M THROWING TOGETHER A FUN POC.
#
# Joel Odom, 2023
#


import openai
import spotipy
from spotipy.oauth2 import SpotifyOAuth
import sys



def get_songs(api_key):
    openai.api_key = api_key

    response = openai.Completion.create(
        engine='text-davinci-003',
        prompt='Generate a Python list of ten random songs where each element of the list is a tuple consisting of the song name and the artist. Don\'t name the list or assign it to a variable, just return the literal list. Make sure to escape any apostrophes.',
        max_tokens=1000,
        temperature=0.7,
        n=1,
        stop=None
    )

    returned_text = response.choices[0].text
    print(returned_text)
    playlist = eval(returned_text) # NOT SAFE OR ROBUST

    response = openai.Completion.create(
        engine='text-davinci-003',
        prompt='Now give the playlist a good name based on the songs it contains.',
        max_tokens=1000,
        temperature=0.7,
        n=1,
        stop=None
    )


    playlist_name = response.choices[0].text.strip().replace('"', '')
    print('Name: ' + playlist_name)
    
    return (playlist_name, playlist)



def get_track_uri(sp, song_name, artist_name):
    query = f"track:{song_name} artist:{artist_name}"
    results = sp.search(q=query, type='track', limit=1)

    if results['tracks']['items']:
        track_uri = results['tracks']['items'][0]['uri']
        return track_uri
    else:
        return None



def create_spotify_playlist(sp, playlist_name, track_uris):
    print(track_uris)
    
    user_id = sp.me()['id']
    playlist = sp.user_playlist_create(user_id, playlist_name, public=False)
    print('Playlist created')

    playlist_id = playlist['id']
    sp.playlist_add_items(playlist_id, track_uris)
    print('Songs added.')

    print(f'Created playlist {playlist_name} with {len(track_uris)} songs.')



def main():
    if len(sys.argv) < 4:
        print("Usage: python chatgpt-playlist.py YOUR_API_KEY YOUR_CLIENT_ID YOUR_CLIENT_SECRET")
        exit(-1)

    api_key = sys.argv[1]
    spotify_client_id = sys.argv[2]
    spotify_client_secret = sys.argv[3]

    try:
        sp = spotipy.Spotify(auth_manager=SpotifyOAuth(client_id=spotify_client_id,
                                                       client_secret=spotify_client_secret,
                                                       redirect_uri='http://localhost:3000',
                                                       scope='playlist-modify-private'))

        track_uris = []
        playlist_name, songs = get_songs(api_key)
        for song in songs:
            print(f"Song: {song[0]}\nArtist: {song[1]}\n")
            uri = get_track_uri(sp, song[0], song[1])
            if uri is not None:
                print(uri)
                track_uris.append(uri)

        create_spotify_playlist(sp, playlist_name, track_uris)

    except spotipy.SpotifyException as e:
        print("An error occurred with the Spotify API:")
        print(e)

    except requests.exceptions.RequestException as e:
        print("A network connection error occurred:")
        print(e)

if __name__ == '__main__':
    main()

