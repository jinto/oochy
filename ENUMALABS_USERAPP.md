<!DOCTYPE html>
<html lang="ko">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Login - Enuma Labs</title>
    <link rel="preconnect" href="https://fonts.googleapis.com">
    <link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap" rel="stylesheet">
    <script src="https://cdn.tailwindcss.com"></script>
    <script>
    tailwind.config = {
        theme: {
            extend: {
                fontFamily: { sans: ['Inter', 'system-ui', 'sans-serif'] },
            }
        }
    }
    </script>
    <script src="https://unpkg.com/htmx.org@2.0.4"></script>
    
    
</head>
<body class="bg-gray-50 text-gray-900 font-sans min-h-screen flex flex-col">
    <!-- Navigation Bar -->
    

    <!-- Messages -->
    

    <!-- Content -->
    <main class="flex-1">
        
<div class="flex items-center justify-center min-h-[80vh]">
    <div class="max-w-sm w-full px-6">
        <div class="text-center mb-8">
            <h1 class="text-2xl font-bold text-gray-900">Enuma Labs</h1>
            <p class="text-sm text-gray-500 mt-2">Sign in to continue</p>
        </div>

        <!-- Google Login -->
        <a href="/accounts/google/login/?next=%2Fdocs%2Fdownload%2F"
           class="flex items-center justify-center gap-3 w-full bg-black text-white rounded-md py-3 px-4 text-sm font-medium hover:bg-gray-800 transition">
            <svg class="w-5 h-5" viewBox="0 0 24 24" fill="none">
                <path d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z" fill="#4285F4"/>
                <path d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z" fill="#34A853"/>
                <path d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z" fill="#FBBC05"/>
                <path d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z" fill="#EA4335"/>
            </svg>
            Continue with Google
        </a>

        <!-- Divider -->
        <div class="relative my-6">
            <div class="absolute inset-0 flex items-center"><div class="w-full border-t border-gray-200"></div></div>
            <div class="relative flex justify-center"><span class="bg-gray-50 px-3 text-xs text-gray-400">or</span></div>
        </div>

        <!-- Email Login -->
        <form method="post" action="/accounts/login/" class="space-y-4">
            <input type="hidden" name="csrfmiddlewaretoken" value="Gv6ZGz9ODzSpPZ7oRemtv62XTRKaQeTYY1qy6yPmJEvp3tIGsvYCdl0Fh39mr85v">

            

            <div>
                <label for="id_login" class="block text-sm font-medium text-gray-700 mb-1">Email</label>
                <input type="email" name="login" id="id_login" placeholder="your.email@enuma.com" required autofocus
                       class="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-black focus:border-transparent">
                
            </div>

            <div>
                <label for="id_password" class="block text-sm font-medium text-gray-700 mb-1">Password</label>
                <input type="password" name="password" id="id_password" placeholder="Enter password" required
                       class="w-full px-3 py-2 border border-gray-300 rounded-md text-sm focus:outline-none focus:ring-2 focus:ring-black focus:border-transparent">
                
            </div>

            <div class="flex items-center">
                <input type="checkbox" name="remember" id="id_remember" class="rounded border-gray-300 mr-2">
                <label for="id_remember" class="text-sm text-gray-600">Remember me</label>
            </div>

            
            <input type="hidden" name="next" value="/docs/download/">
            

            <button type="submit"
                    class="w-full bg-gray-900 text-white rounded-md py-2.5 text-sm font-medium hover:bg-black transition">
                Sign In
            </button>
        </form>

        
        <p class="text-center text-xs text-gray-400 mt-4">You'll be redirected after login.</p>
        
    </div>
</div>

    </main>

    <!-- Footer -->
    <footer class="border-t border-gray-200 py-6 mt-12">
        <div class="max-w-6xl mx-auto px-4 text-center text-xs text-gray-400">
            Enuma Labs &copy; 2025
        </div>
    </footer>
</body>
</html>
