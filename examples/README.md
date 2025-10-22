# This-RS Examples

Ce dossier contient des exemples progressifs pour apprendre à utiliser This-RS.

## 📚 Exemples Disponibles

### 1. 🎯 [simple_api](./simple_api/) - Les Bases

**Niveau** : Débutant  
**Durée** : 5 minutes  
**Fichiers** : 1 fichier

Démarrez ici ! Exemple minimal montrant :
- Définition d'entités
- Création de liens
- Navigation bidirectionnelle

```bash
cargo run --example simple_api
```

### 2. 🌐 [full_api](./full_api/) - Serveur HTTP

**Niveau** : Intermédiaire  
**Durée** : 15 minutes  
**Fichiers** : 1 fichier

Serveur HTTP complet avec :
- Configuration YAML
- Routes HTTP auto-générées
- Registry de routes
- Introspection d'API

```bash
cargo run --example full_api
# Serveur sur http://127.0.0.1:3000
```

### 3. 🚀 [microservice](./microservice/) - Production Ready

**Niveau** : Avancé  
**Durée** : 30 minutes  
**Fichiers** : 5 fichiers modulaires

Microservice complet avec architecture professionnelle :
- Structure modulaire (entities/store/handlers/module)
- Routes CRUD complètes
- Module trait
- Authorization policies
- Prêt pour ScyllaDB

```bash
cargo run --example microservice
# Serveur sur http://127.0.0.1:3000
```

## 🎓 Parcours d'Apprentissage Recommandé

### Étape 1 : Comprendre les Concepts (simple_api)
1. Lire `simple_api/README.md`
2. Lancer `cargo run --example simple_api`
3. Examiner le code dans `simple_api/main.rs`
4. Comprendre Entity, Link, EntityReference

### Étape 2 : API HTTP (full_api)
1. Lire `full_api/README.md`
2. Lancer `cargo run --example full_api`
3. Tester les routes avec curl
4. Comprendre LinkRouteRegistry, Handlers

### Étape 3 : Architecture Microservice (microservice)
1. Lire `microservice/README.md`
2. Explorer la structure modulaire
3. Lancer le serveur
4. Tester toutes les routes CRUD et liens
5. Lire `ARCHITECTURE_MICROSERVICES.md` pour aller plus loin

## 📊 Comparaison des Exemples

| Feature | simple_api | full_api | microservice |
|---------|:----------:|:--------:|:------------:|
| Entités définies | ✅ | ✅ | ✅ |
| Liens bidirectionnels | ✅ | ✅ | ✅ |
| Serveur HTTP | ❌ | ✅ | ✅ |
| Routes CRUD | ❌ | ❌ | ✅ |
| Configuration YAML | ❌ | ✅ | ✅ |
| Structure modulaire | ❌ | ❌ | ✅ |
| Auth policies | ❌ | ❌ | ✅ |
| Module trait | ❌ | ❌ | ✅ |
| Store abstrait | ❌ | ❌ | ✅ |
| Production-ready | ❌ | ❌ | ✅ |

## 🔧 Tests Rapides

```bash
# Compiler tous les exemples
cargo build --examples

# Lancer un exemple spécifique
cargo run --example simple_api
cargo run --example full_api
cargo run --example microservice

# Tester avec curl (full_api ou microservice)
curl http://127.0.0.1:3000/orders
curl http://127.0.0.1:3000/invoices
```

## 📖 Documentation Associée

- **README.md** - Vue d'ensemble du projet
- **START_HERE.md** - Point d'entrée principal
- **GETTING_STARTED.md** - Guide complet
- **ARCHITECTURE_MICROSERVICES.md** - Architecture détaillée
- **IMPLEMENTATION_COMPLETE.md** - Résumé des features

## 🎯 Objectifs Pédagogiques

### simple_api
- ✅ Comprendre Entity et Data traits
- ✅ Créer et naviguer des liens
- ✅ Utiliser InMemoryLinkService

### full_api
- ✅ Configuration YAML
- ✅ Routes HTTP génériques
- ✅ LinkRouteRegistry
- ✅ Handlers Axum

### microservice
- ✅ Architecture modulaire professionnelle
- ✅ Séparation des responsabilités
- ✅ CRUD complet
- ✅ Auth policies
- ✅ Trait Module
- ✅ Pattern prêt pour ScyllaDB

## 🚀 Après les Exemples

Une fois les exemples maîtrisés, vous pouvez :

1. **Créer votre propre microservice**
   - Utiliser `microservice/` comme template
   - Définir vos entités dans `entities.rs`
   - Implémenter votre store

2. **Migrer vers ScyllaDB**
   - Suivre `ARCHITECTURE_MICROSERVICES.md`
   - Implémenter `ScyllaDBLinkService`
   - Implémenter votre store ScyllaDB

3. **Ajouter l'authentification**
   - Implémenter `JwtAuthProvider`
   - Intégrer les auth policies
   - Middleware d'autorisation

4. **Features avancées**
   - Pagination
   - Filtres et tri
   - Caching (Redis/LRU)
   - Rate limiting
   - Monitoring (Prometheus)

## 💡 Conseils

### Pour Débutants
- Commencez par `simple_api`
- Ne sautez pas d'étapes
- Lisez les commentaires dans le code
- Testez chaque exemple

### Pour Développeurs Expérimentés
- Allez directement à `microservice`
- Lisez `ARCHITECTURE_MICROSERVICES.md`
- Examinez le code modulaire
- Adaptez à vos besoins

### Pour Production
- Utilisez `microservice` comme base
- Implémentez ScyllaDB
- Ajoutez JWT auth
- Suivez les checklist dans `ARCHITECTURE_MICROSERVICES.md`

## 🐛 Troubleshooting

### Exemple ne compile pas
```bash
cargo clean
cargo build --example <nom>
```

### Port déjà utilisé
```bash
# Changer le port dans main.rs
let listener = tokio::net::TcpListener::bind("127.0.0.1:3001")
```

### Erreur de runtime
- Vérifier que le serveur est lancé
- Vérifier les UUIDs dans les commandes curl
- Vérifier les headers (X-Tenant-ID)

## 📞 Support

- **Issues** : (Votre repo GitHub)
- **Discussions** : (Votre forum)
- **Documentation** : Lire `START_HERE.md`

---

**Bonne exploration !** 🦀✨

